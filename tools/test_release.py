"""Exercise release rejection paths without credentials or network requests."""
import os
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


if __name__ == "__main__":
  unittest.main()
