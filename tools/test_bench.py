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

  def test_different_transports_do_not_overwrite_each_other(self):
    rows = [{"case": "same", "transport": transport, "tls_provider": "jdk",
             "median_requests_per_second": rps, "max_peak_rss_bytes": None,
             "samples": [{"requests": 100}]} for transport, rps in (("bemo", 100), ("epoll", 1000))]
    current = copy.deepcopy(rows)
    current[0]["median_requests_per_second"] = 60
    self.assertEqual(len(compare(current, rows)), 1)

  def test_transport_comparison_requires_matching_tls_and_load(self):
    sample = {"clients": 4, "requests": 100, "warmup_rounds": 20, "tls_provider": "jdk",
              "java_version": "stock", "host": "same", "workload_sha256": "same",
              "requests_per_second": 100}
    rows = [{"case": "plain", "transport": transport, "tls_provider": "jdk",
             "median_requests_per_second": 100, "median_latency_p99_ns": 10,
             "samples": [copy.deepcopy(sample)]} for transport in ("bemo", "epoll")]
    self.assertEqual(bench.comparisons(rows)[0]["bemo_over_comparator_rps"], 1)
    rows[1]["samples"][0]["tls_provider"] = "other"
    with self.assertRaisesRegex(RuntimeError, "incompatible"):
      bench.comparisons(rows)


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

  def test_lockfile_changes_are_never_excluded_from_source_hash(self):
    # The rename failure came from Cargo normalizing package order in Cargo.lock.
    # A real lock edit must still invalidate preparation rather than be ignored.
    with tempfile.TemporaryDirectory() as temporary:
      root = Path(temporary)
      lock = root / "Cargo.lock"
      lock.write_text('version = 4\n')
      with patch.object(bench.build, "ROOT", root):
        before = bench.digest([lock])
        lock.write_text('version = 4\n# dependency changed\n')
        self.assertNotEqual(bench.digest([lock]), before)


if __name__ == "__main__":
  unittest.main()
