"""Exercise Central signing failures and the validation/publication boundary offline."""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import publish_central as central


class CentralPublishingTest(unittest.TestCase):
  def repository(self, source, destination, value, *, include_thinlto=True):
    names = set().union(*(central.packages.artifacts(value, platform, include_thinlto=include_thinlto)
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


  def test_older_release_without_thinlto_is_complete(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      with patch.object(central.packages, "merge", side_effect=self.repository), \
           patch.object(central.subprocess, "run", side_effect=self.sign):
        bundle = central.prepare(root, root / "bundle.zip", "0.2.0", "fingerprint", include_thinlto=False)
        central.validate_bundle(bundle, "0.2.0")
        with zipfile.ZipFile(bundle) as archive:
          self.assertFalse(any("-thinlto.jar" in name for name in archive.namelist()))
          self.assertEqual(sum(name.endswith((".jar", ".pom")) for name in archive.namelist()), 20)
        with zipfile.ZipFile(bundle, "a") as archive:
          name = next(name for name in central.packages.artifacts("0.2.0", "linux-x86_64-gnu") if name.endswith("-thinlto.jar"))
          archive.writestr(name, b"incomplete optional classifier pair")
        with self.assertRaisesRegex(RuntimeError, "unexpected"):
          central.validate_bundle(bundle, "0.2.0")

  def test_signing_passphrase_uses_stdin(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      with patch.object(central.packages, "merge", side_effect=self.repository), \
           patch.object(central.subprocess, "run", side_effect=self.sign) as run, \
           patch.dict(os.environ, {"BEMO_PGP_PASSPHRASE": "not-in-arguments"}):
        central.prepare(root, root / "bundle.zip", "0.2.0", "fingerprint")
      signing = [call for call in run.call_args_list if "--detach-sign" in call.args[0]]
      self.assertTrue(signing)
      for call in signing:
        self.assertNotIn("not-in-arguments", call.args[0])
        self.assertIn("--passphrase-fd", call.args[0])
        self.assertEqual(call.kwargs["input"], b"not-in-arguments")

  def test_wait_handles_validation_publication_failure_and_timeout(self):
    deployment = "28570f16-da32-4c14-bd2e-c1acc0782365"
    with patch.object(central, "status", side_effect=[{"deploymentState": state} for state in ("PENDING", "VALIDATING", "VALIDATED")]), \
         patch.object(central.time, "sleep"):
      self.assertEqual(central.wait(deployment, "VALIDATED")["deploymentState"], "VALIDATED")
    with patch.object(central, "status", side_effect=[{"deploymentState": state} for state in ("PUBLISHING", "PUBLISHED")]), \
         patch.object(central.time, "sleep"):
      self.assertEqual(central.wait(deployment, "PUBLISHED")["deploymentState"], "PUBLISHED")
    with patch.object(central, "status", return_value={"deploymentState": "FAILED", "errors": {"namespace": "not verified"}}):
      with self.assertRaisesRegex(RuntimeError, "not verified"):
        central.wait(deployment, "VALIDATED")
    with patch.object(central, "status", return_value={"deploymentState": "VALIDATING"}), \
         patch.object(central.time, "monotonic", side_effect=(0, 2)):
      with self.assertRaises(TimeoutError):
        central.wait(deployment, "VALIDATED", timeout=1)
    with patch.object(central, "status", return_value={"deploymentState": "VALIDATED"}):
      with self.assertRaisesRegex(RuntimeError, "not been requested"):
        central.wait(deployment, "PUBLISHED")

  def release_fixture(self):
    value = "0.2.0"
    payload = central.packages.release.expected_assets(value)
    files = {name: name.encode() for name in payload}
    files["SHA256SUMS"] = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, data in sorted(files.items())).encode()
    files.update({name + ".sigstore.json": b"sigstore" for name in list(files)})
    release = {"immutable": True, "draft": False, "prerelease": False,
               "html_url": "https://github.com/elide-dev/bemo/releases/tag/v0.2.0",
               "assets": [{"name": name, "digest": "sha256:" + hashlib.sha256(data).hexdigest()} for name, data in files.items()]}
    return release, files

  def test_fetch_authenticates_existing_release_without_building(self):
    release, files = self.release_fixture()
    def execute(command, **kwargs):
      if command[:3] == ["gh", "release", "download"]:
        destination = Path(command[command.index("--dir") + 1])
        for name, data in files.items():
          (destination / name).write_bytes(data)
    commit = "a" * 40
    with tempfile.TemporaryDirectory() as directory, \
         patch.object(central.packages.release, "gh", side_effect=(json.dumps(release), json.dumps({"object": {"type": "commit", "sha": commit}}))), \
         patch.object(central.subprocess, "run", side_effect=execute) as run:
      manifest = central.fetch_release(Path(directory) / "verified", "0.2.0")
      self.assertEqual(manifest["source_commit"], commit)
      provenance = [call.args[0] for call in run.call_args_list if call.args[0][:3] == ["gh", "attestation", "verify"]]
      self.assertEqual(len(provenance), 2)
      for command in provenance:
        self.assertIn(commit, command)
        self.assertIn("refs/heads/main", command)
        self.assertIn("--deny-self-hosted-runners", command)
      self.assertFalse(any(call.args[0][0] in ("cargo", "elide", "make") for call in run.call_args_list))

  def test_fetch_rejects_mutable_releases_and_changed_asset_bytes(self):
    release, files = self.release_fixture()
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      release["immutable"] = False
      with patch.object(central.packages.release, "gh", return_value=json.dumps(release)), \
           patch.object(central.subprocess, "run") as run:
        with self.assertRaisesRegex(RuntimeError, "immutable"):
          central.fetch_release(root / "verified", "0.2.0")
        run.assert_not_called()
      release["immutable"] = True
      def changed_download(command, **kwargs):
        destination = Path(command[command.index("--dir") + 1])
        for name, data in files.items():
          (destination / name).write_bytes(data + b"tampered")
      with patch.object(central.packages.release, "gh", side_effect=(json.dumps(release), json.dumps({"object": {"type": "commit", "sha": "a" * 40}}))), \
           patch.object(central.subprocess, "run", side_effect=changed_download):
        with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
          central.fetch_release(root / "verified", "0.2.0")
      self.assertFalse((root / "verified").exists())


if __name__ == "__main__":
  unittest.main()
