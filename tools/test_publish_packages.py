"""Validate cross-platform package merging and deployment without credentials."""
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import publish_packages as packages


class PackagePublishingTest(unittest.TestCase):
  def bundles(self, source, changed=None):
    for platform in packages.release.PLATFORMS.values():
      contents = {}
      for name in packages.artifacts("0.1.0-SNAPSHOT", platform):
        if name.endswith(".pom"):
          artifact = name.split("/")[-3]
          content = (f'<project xmlns="http://maven.apache.org/POM/4.0.0"><groupId>dev.elide.bemo</groupId>'
                     f'<artifactId>{artifact}</artifactId><version>0.1.0-SNAPSHOT</version>'
                     '<packaging>jar</packaging></project>').encode()
        else:
          content = name.encode()
        contents[name] = content
      if changed:
        changed(contents, platform)
      with zipfile.ZipFile(source / f"bemo-0.1.0-SNAPSHOT-{platform}-unsigned.zip", "w") as archive:
        for name, content in contents.items():
          archive.writestr(name, content)
          for algorithm in packages.CHECKSUMS:
            archive.writestr(f"{name}.{algorithm}", hashlib.new(algorithm, content).hexdigest() + "\n")

  def test_merge_retains_every_native_classifier_and_common_artifact(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      self.bundles(root)
      output = packages.merge(root, root / "merged", "0.1.0-SNAPSHOT")
      expected = set().union(*(packages.artifacts("0.1.0-SNAPSHOT", platform)
                               for platform in packages.release.PLATFORMS.values()))
      self.assertEqual({str(p.relative_to(output)) for p in output.rglob("*") if p.is_file()}, expected)

  def test_common_artifact_difference_rejects_before_output_changes(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      def change(contents, platform):
        if platform == "osx-aarch64":
          name = next(n for n in contents if n.endswith("-sources.jar"))
          contents[name] = b"different sources"
      self.bundles(root, change)
      output = root / "merged"
      output.mkdir()
      sentinel = output / "existing"
      sentinel.write_text("keep")
      with self.assertRaisesRegex(RuntimeError, "differ between platforms"):
        packages.merge(root, output, "0.1.0-SNAPSHOT")
      self.assertEqual(sentinel.read_text(), "keep")

  def test_invalid_checksum_and_unexpected_paths_are_rejected(self):
    for corruption in ("checksum", "unexpected"):
      with self.subTest(corruption=corruption), tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        self.bundles(root)
        bundle = root / "bemo-0.1.0-SNAPSHOT-linux-x86_64-gnu-unsigned.zip"
        with zipfile.ZipFile(bundle) as archive:
          contents = {name: archive.read(name) for name in archive.namelist()}
        if corruption == "checksum":
          name = next(n for n in contents if n.endswith(".sha256"))
          contents[name] = b"invalid"
        else:
          contents["../outside"] = b"invalid"
        with zipfile.ZipFile(bundle, "w") as archive:
          for name, content in contents.items():
            archive.writestr(name, content)
        with self.assertRaises(RuntimeError):
          packages.merge(root, root / "merged", "0.1.0-SNAPSHOT")
        self.assertFalse((root / "merged").exists())

  def test_deploy_attaches_sources_javadocs_and_both_native_platforms(self):
    with patch.object(packages.subprocess, "run") as run:
      packages.deploy(Path("repository"), "0.1.0-SNAPSHOT", Path("settings.xml"))
    self.assertEqual(run.call_count, 4)
    for call in run.call_args_list:
      command = call.args[0]
      classifiers = next(arg for arg in command if arg.startswith("-Dclassifiers="))
      self.assertIn("sources,javadoc", classifiers)
      if any("-Dfile=repository/dev/elide/bemo/bemo-ffm/" in arg or
             "-Dfile=repository/dev/elide/bemo/bemo-native-image/" in arg for arg in command):
        self.assertIn("linux-x86_64-gnu", classifiers)
        self.assertIn("osx-aarch64", classifiers)

  def test_publication_rejects_pr_context_and_keeps_token_out_of_settings(self):
    environment = {"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/heads/main",
                   "GITHUB_REPOSITORY": "elide-dev/bemo", "GH_TOKEN": "private-token", "GITHUB_ACTOR": "actor"}
    with patch.dict(os.environ, environment), patch.object(packages, "deploy") as deploy:
      with self.assertRaisesRegex(RuntimeError, "verified main push"):
        packages.publish(Path("repository"), "0.1.0-SNAPSHOT")
      deploy.assert_not_called()
    environment["GITHUB_EVENT_NAME"] = "push"
    def check_settings(repository, value, settings):
      self.assertNotIn("private-token", settings.read_text())
      self.assertIn("${env.GH_TOKEN}", settings.read_text())
      self.assertEqual(settings.stat().st_mode & 0o777, 0o600)
    with patch.dict(os.environ, environment), patch.object(packages, "deploy", side_effect=check_settings), \
         patch.object(packages, "verify_remote") as verify:
      packages.publish(Path("repository"), "0.1.0-SNAPSHOT")
      verify.assert_called_once()

  def test_snapshot_verification_resolves_timestamped_names_and_rejects_changed_bytes(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      self.bundles(root)
      output = packages.merge(root, root / "merged", "0.1.0-SNAPSHOT")
      remote = {}
      for module in packages.MODULES:
        artifact = f"bemo-{module}"
        path = f"dev/elide/bemo/{artifact}/0.1.0-SNAPSHOT"
        entries = []
        for file in (output / path).iterdir():
          classifier = file.stem[len(f"{artifact}-0.1.0-SNAPSHOT"):].removeprefix("-")
          suffix = f"-{classifier}" if classifier else ""
          name = f"{artifact}-0.1.0-20261006.000000-1{suffix}{file.suffix}"
          remote[f"{packages.REGISTRY}/{path}/{name}"] = file.read_bytes()
          entries.append(f'<snapshotVersion><extension>{file.suffix[1:]}</extension>'
                         f'<classifier>{classifier}</classifier><value>0.1.0-20261006.000000-1</value></snapshotVersion>')
        remote[f"{packages.REGISTRY}/{path}/maven-metadata.xml"] = (
            '<metadata><versioning><snapshotVersions>' + ''.join(entries) +
            '</snapshotVersions></versioning></metadata>').encode()
      with patch.object(packages, "read_remote", side_effect=lambda url, headers: remote[url]):
        packages.verify_remote(output, "0.1.0-SNAPSHOT", {})
        jar = next(url for url in remote if url.endswith(".jar"))
        remote[jar] = b"changed"
        with self.assertRaisesRegex(RuntimeError, "differs from verified staging"):
          packages.verify_remote(output, "0.1.0-SNAPSHOT", {})

  def test_existing_release_version_is_not_republished_from_later_main_commits(self):
    with tempfile.TemporaryDirectory() as directory:
      output = Path(directory) / "output"
      with patch.object(packages.build, "VERSION", "0.2.0"), \
           patch.object(packages.release, "release_tag", return_value="v0.2.0"), \
           patch.object(packages.release, "gh", return_value='{"object":{"type":"commit","sha":"released"}}'), \
           patch.dict(os.environ, {"GITHUB_REPOSITORY": "elide-dev/bemo", "GITHUB_SHA": "later",
                                   "GITHUB_OUTPUT": str(output)}):
        packages.select()
      self.assertEqual(output.read_text(), "publish=false\n")


if __name__ == "__main__":
  unittest.main()
