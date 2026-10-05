import copy
import unittest
import tempfile
from pathlib import Path
from unittest.mock import patch
import bench
from compare_bench import compare


class ComparisonTests(unittest.TestCase):
  def test_regressions_and_incompatible_workloads(self):
    baseline = [{"case": "plain", "median_requests_per_second": 1000,
                 "max_peak_rss_bytes": 100, "samples": [{"requests": 100, "host": "same"}]}]
    current = copy.deepcopy(baseline)
    current[0].update(median_requests_per_second=700, max_peak_rss_bytes=130)
    self.assertEqual(len(compare(current, baseline)), 2)
    current[0]["samples"][0]["requests"] = 200
    self.assertIn("skipped", compare(current, baseline)[0])
    self.assertEqual(compare(baseline, baseline), [])


class ManifestTests(unittest.TestCase):
  def test_generated_reports_do_not_change_inputs_but_sources_and_artifacts_do(self):
    with tempfile.TemporaryDirectory() as temporary:
      root = Path(temporary)
      build_dir = root / "build"
      source = root / "crates/bemo/src/lib.rs"
      classes = build_dir / "classes/api/Test.class"
      library = root / "target/release/libbemo.so"
      inputs = [root / name for name in ("Cargo.toml", "Cargo.lock", "elide.pkl",
                "tools/versions.json", "rust-toolchain.toml")] + [source, classes, library]
      for path in inputs:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"original")
      with patch.object(bench.build, "ROOT", root), patch.object(bench.build, "BUILD", build_dir), \
           patch.object(bench.build, "library", return_value=library):
        baseline = bench.snapshot()
        report = root / "crates/bemo/target/criterion/tls/new/estimates.json"
        report.parent.mkdir(parents=True)
        report.write_text('{"mean": 123}')
        self.assertEqual(bench.snapshot(), baseline)
        report.write_text('{"mean": 456}')
        self.assertEqual(bench.snapshot(), baseline)
        for path, category in [(source, "sources_sha256"), (classes, "classes_sha256"),
                               (library, "library_sha256")]:
          path.write_bytes(b"changed")
          self.assertNotEqual(bench.snapshot()[category], baseline[category])
          path.write_bytes(b"original")
        source.unlink()
        self.assertNotEqual(bench.snapshot()["sources_sha256"], baseline["sources_sha256"])


if __name__ == "__main__":
  unittest.main()
