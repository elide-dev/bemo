import copy
import unittest
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


if __name__ == "__main__":
  unittest.main()
