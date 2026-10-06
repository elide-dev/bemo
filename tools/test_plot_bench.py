"""Guard against misleading README charts when benchmark inputs change."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

import plot_bench as charts


class PerformanceChartTest(unittest.TestCase):
  def setUp(self):
    self.meta = json.loads(charts.DEFAULT.read_text())
    self.rows = json.loads((charts.DEFAULT.parent / self.meta["summary"]).read_text())

  def load(self, rows):
    with tempfile.TemporaryDirectory() as directory:
      root = Path(directory)
      (root / self.meta["summary"]).write_text(json.dumps(rows))
      provenance = root / "provenance.json"
      provenance.write_text(json.dumps(self.meta))
      return charts.load(provenance)

  def test_raw_samples_are_authoritative(self):
    expected = charts.values(self.load(self.rows)[1], charts.CASES[0], "bemo", "requests_per_second")
    for row in self.rows:
      row["median_requests_per_second"] = -1
    meta, groups = self.load(self.rows)
    self.assertEqual(len(groups), 16)
    self.assertEqual(charts.values(groups, charts.CASES[0], "bemo", "requests_per_second"), expected)
    self.assertEqual(len(meta["sha256"]), 64)

  def test_missing_cases_and_insufficient_samples_are_rejected(self):
    with self.assertRaisesRegex(ValueError, "Incomplete"):
      self.load(self.rows[:-1])
    self.rows[0]["samples"] = self.rows[0]["samples"][:1]
    with self.assertRaisesRegex(ValueError, "three samples"):
      self.load(self.rows)

  def test_mixed_environment_and_backends_are_rejected(self):
    for field, value, message in (("clients", 8, "Mixed benchmark"),
                                  ("commit", "wrong", "Mixed benchmark"),
                                  ("driver", "epoll", "Mixed transport"),
                                  ("auto_fallback", True, "AUTO fallback"),
                                  ("http_provider", "netty", "Unexpected benchmark stack")):
      with self.subTest(field=field):
        rows = copy.deepcopy(self.rows)
        rows[0]["samples"][1][field] = value
        with self.assertRaisesRegex(ValueError, message):
          self.load(rows)

  def test_invalid_metrics_are_rejected(self):
    for value in (None, 0, -1, float("nan"), float("inf")):
      with self.subTest(value=value):
        rows = copy.deepcopy(self.rows)
        rows[0]["samples"][0]["peak_rss_bytes"] = value
        with self.assertRaisesRegex(ValueError, "Invalid benchmark metric"):
          self.load(rows)


if __name__ == "__main__":
  unittest.main()
