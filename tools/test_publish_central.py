"""Exercise Central signing failures and the validation/publication boundary offline."""
from pathlib import Path
import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import publish_central as central


class CentralPublishingTest(unittest.TestCase):
  def repository(self, source, destination, value):
    names = set().union(*(central.packages.artifacts(value, platform)
                         for platform in central.packages.release.PLATFORMS.values()))
    for name in names:
      path = destination / name
      path.parent.mkdir(parents=True, exist_ok=True)
      path.write_bytes(name.encode())
    return destination

  def sign(self, command, **kwargs):
    if "--detach-sign" in command:
      Path(command[command.index("--output") + 1]).write_text("test-signature")

  def test_complete_bundle_retains_classifiers_signatures_and_checksums(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      with patch.object(central.packages, "merge", side_effect=self.repository), \
           patch.object(central.subprocess, "run", side_effect=self.sign):
        bundle = central.prepare(root, root / "bundle.zip", "0.2.0", "fingerprint")
        central.validate_bundle(bundle, "0.2.0")
        with zipfile.ZipFile(bundle) as archive:
          self.assertTrue(any("-thinlto.jar.asc" in name for name in archive.namelist()))
          self.assertTrue(any("-sources.jar.asc" in name for name in archive.namelist()))
          self.assertTrue(all(name.startswith("dev/") for name in archive.namelist()))
        with zipfile.ZipFile(bundle, "a") as archive:
          archive.writestr("../escape", b"invalid")
        with self.assertRaisesRegex(RuntimeError, "unexpected"):
          central.validate_bundle(bundle, "0.2.0")

  @unittest.skipUnless(shutil.which("gpg"), "GPG is not installed")
  def test_real_gpg_signatures_reject_tampered_artifacts(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      home = root / "gnupg"
      home.mkdir(mode=0o700)
      with patch.dict(os.environ, {"GNUPGHOME": str(home)}):
        subprocess.run(["gpg", "--batch", "--pinentry-mode", "loopback", "--passphrase", "",
                        "--quick-generate-key", "Bemo Test <test@example.invalid>", "ed25519", "sign", "1d"],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
          with patch.object(central.packages, "merge", side_effect=self.repository):
            bundle = central.prepare(root, root / "bundle.zip", "0.2.0", "test@example.invalid")
          with zipfile.ZipFile(bundle) as archive:
            contents = {name: archive.read(name) for name in archive.namelist()}
          name = next(name for name in contents if name.endswith(".jar"))
          contents[name] = b"tampered"
          for algorithm in central.packages.CHECKSUMS:
            contents[name + "." + algorithm] = (hashlib.new(
                algorithm, contents[name]).hexdigest() + "\n").encode()
          with zipfile.ZipFile(bundle, "w") as archive:
            for name, content in contents.items():
              archive.writestr(name, content)
          with self.assertRaises(subprocess.CalledProcessError):
            central.validate_bundle(bundle, "0.2.0")
        finally:
          subprocess.run(["gpgconf", "--kill", "gpg-agent"], check=True)

  def test_snapshots_and_signing_failures_preserve_existing_bundle(self):
    with tempfile.TemporaryDirectory() as directory:
      bundle = Path(directory) / "bundle.zip"
      bundle.write_bytes(b"keep")
      with self.assertRaises(ValueError):
        central.prepare(bundle.parent, bundle, "0.2.0-SNAPSHOT", "key")
      with patch.object(central.packages, "merge", side_effect=self.repository), \
           patch.object(central.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "gpg")):
        with self.assertRaises(subprocess.CalledProcessError):
          central.prepare(bundle.parent, bundle, "0.2.0", "key")
      self.assertEqual(bundle.read_bytes(), b"keep")

  def test_upload_is_user_managed_and_never_publishes(self):
    deployment = "28570f16-da32-4c14-bd2e-c1acc0782365"
    with tempfile.TemporaryDirectory() as directory:
      bundle = Path(directory) / "bundle.zip"
      bundle.write_bytes(b"bundle")
      with patch.object(central, "validate_bundle") as validate, \
           patch.object(central, "request", return_value=deployment) as request:
        self.assertEqual(central.upload(bundle, "0.2.0"), deployment)
        validate.assert_called_once_with(bundle, "0.2.0")
        self.assertIn("publishingType=USER_MANAGED", request.call_args.args[0])
        self.assertIn(b'name="bundle"', request.call_args.args[1])
        self.assertEqual(request.call_count, 1)

  def test_remote_verification_rejects_changed_artifacts(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      with patch.object(central.packages, "merge", side_effect=self.repository), \
           patch.object(central.subprocess, "run", side_effect=self.sign):
        bundle = central.prepare(root, root / "bundle.zip", "0.2.0", "fingerprint")
        with zipfile.ZipFile(bundle) as archive:
          def remote(url, headers):
            return archive.read(url.removeprefix("https://repo.maven.apache.org/maven2/"))
          with patch.object(central.packages, "read_remote", side_effect=remote):
            central.verify_remote(bundle, "0.2.0")
        with patch.object(central.packages, "read_remote", return_value=b"changed"):
          with self.assertRaisesRegex(RuntimeError, "differs"):
            central.verify_remote(bundle, "0.2.0")

  def test_publication_requires_validated_deployment(self):
    deployment = "28570f16-da32-4c14-bd2e-c1acc0782365"
    for state in ("PENDING", "FAILED", "PUBLISHED", "VALIDATED"):
      with self.subTest(state=state), patch.object(central, "status", return_value={"deploymentState": state}), \
           patch.object(central, "request") as request:
        if state == "VALIDATED":
          central.publish(deployment)
          request.assert_called_once_with("/deployment/" + deployment)
        else:
          with self.assertRaisesRegex(RuntimeError, "VALIDATED"):
            central.publish(deployment)
          request.assert_not_called()


if __name__ == "__main__":
  unittest.main()
