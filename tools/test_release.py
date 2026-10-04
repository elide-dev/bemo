"""Exercise release rejection paths without credentials or network requests."""
import os
import json
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import release


class ReleaseSafetyTest(unittest.TestCase):
  def test_snapshots_never_select_a_release(self):
    with patch.object(release, "version", return_value="0.1.0-SNAPSHOT"), patch.object(release, "gh") as gh:
      release.select()
      gh.assert_not_called()

  def test_draft_must_match_tested_commit(self):
    with patch.dict(os.environ, {"GITHUB_REPOSITORY": "elide-dev/dokar", "GITHUB_SHA": "tested"}), \
         patch.object(release, "version", return_value="0.1.0"), \
         patch.object(release, "gh", side_effect=[
             '[[{"tag_name":"v0.1.0","draft":true}]]',
             '{"object":{"type":"commit","sha":"different"}}']):
      with self.assertRaisesRegex(RuntimeError, "tested commit"):
        release.select()

  def test_stage_requires_every_platform_and_provenance(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      (root / "build/release-input").mkdir(parents=True)
      with patch.object(release, "ROOT", root), patch.object(release, "version", return_value="0.1.0"), \
           patch.object(release, "gh") as gh:
        with self.assertRaisesRegex(RuntimeError, "platform release evidence"):
          release.stage()
        gh.assert_not_called()

  def test_publish_requires_complete_signatures(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      (root / "build/release-assets").mkdir(parents=True)
      with patch.object(release, "ROOT", root), patch.object(release, "version", return_value="0.1.0"), \
           patch.dict(os.environ, {"GITHUB_REPOSITORY": "elide-dev/dokar", "RELEASE_TAG": "v0.1.0"}), \
           patch.object(release, "gh") as gh:
        with self.assertRaisesRegex(RuntimeError, "Incomplete signed"):
          release.publish()
        gh.assert_not_called()

  def test_stage_verifies_every_platform_before_copying(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      source = root / "build/release-input"
      source.mkdir(parents=True)
      for name in release.expected_assets("0.1.0"):
        (source / name).write_bytes(name.encode())
      with patch.object(release, "ROOT", root), patch.object(release, "version", return_value="0.1.0"), \
           patch.dict(os.environ, {"GITHUB_REPOSITORY": "elide-dev/dokar", "GITHUB_SHA": "tested"}), \
           patch.object(release, "gh", return_value="verified") as gh:
        release.stage()
        self.assertEqual(gh.call_count, len(release.PLATFORMS))
        for call in gh.call_args_list:
          self.assertIn("--source-digest", call.args)
          self.assertIn("tested", call.args)
          self.assertIn("--deny-self-hosted-runners", call.args)
        destination = root / "build/release-assets"
        for line in (destination / "SHA256SUMS").read_text().splitlines():
          digest, name = line.split("  ")
          self.assertEqual(digest, hashlib.sha256((destination / name).read_bytes()).hexdigest())

  def test_publish_rejects_uploaded_digest_mismatch(self):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      destination = root / "build/release-assets"
      destination.mkdir(parents=True)
      payload = release.expected_assets("0.1.0") | {"SHA256SUMS"}
      names = payload | {name + ".sigstore.json" for name in payload}
      for name in names:
        (destination / name).write_bytes(b"asset")
      replies = ["true", '{"assets":[]}', "uploaded",
                 json.dumps({"assets": [{"name": name, "digest": "sha256:wrong"} for name in names]})]
      with patch.object(release, "ROOT", root), patch.object(release, "version", return_value="0.1.0"), \
           patch.dict(os.environ, {"GITHUB_REPOSITORY": "elide-dev/dokar", "RELEASE_TAG": "v0.1.0"}), \
           patch.object(release, "gh", side_effect=replies) as gh:
        with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
          release.publish()
        self.assertFalse(any("--draft=false" in call.args for call in gh.call_args_list))


  def test_publish_verifies_immutable_release_without_admin_api(self):
    for immutable in (True, False, None):
      with self.subTest(immutable=immutable), tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        destination = root / "build/release-assets"
        destination.mkdir(parents=True)
        payload = release.expected_assets("0.1.0") | {"SHA256SUMS"}
        names = payload | {name + ".sigstore.json" for name in payload}
        for name in names:
          (destination / name).write_bytes(b"asset")
        digest = "sha256:" + hashlib.sha256(b"asset").hexdigest()
        replies = ["true", '{"assets":[]}', "uploaded",
                   json.dumps({"assets": [{"name": name, "digest": digest} for name in names]}),
                   "published", json.dumps({} if immutable is None else {"immutable": immutable})]
        with patch.object(release, "ROOT", root), patch.object(release, "version", return_value="0.1.0"), \
             patch.dict(os.environ, {"GITHUB_REPOSITORY": "elide-dev/dokar", "RELEASE_TAG": "v0.1.0"}), \
             patch.object(release, "gh", side_effect=replies) as gh:
          if immutable is True:
            release.publish()
          else:
            with self.assertRaisesRegex(RuntimeError, "not immutable"):
              release.publish()
          calls = [call.args for call in gh.call_args_list]
          self.assertFalse(any("immutable-releases" in arg for call in calls for arg in call))
          self.assertEqual(calls[-2], ("release", "edit", "v0.1.0", "--draft=false", "--repo", "elide-dev/dokar"))
          self.assertEqual(calls[-1], ("api", "repos/elide-dev/dokar/releases/tags/v0.1.0"))


if __name__ == "__main__":
  unittest.main()
