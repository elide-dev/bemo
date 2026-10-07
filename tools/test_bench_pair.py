"""Preserve raw sample authority while merging alternating benchmark invocations."""
import unittest

import bench_pair


class PairedBenchmarkAggregationTest(unittest.TestCase):
  def test_samples_stay_separate_and_aggregates_use_raw_measurements(self):
    rows = []
    for transport in ('bemo', 'epoll'):
      for rps, p99, server, client, rss in [(100, 30, 30, 10, 200),
                                          (300, 10, 10, 30, 100),
                                          (200, 20, 20, 20, 300)]:
        rows.append({'case': 'plain-identity-1024', 'transport': transport,
                     'median_requests_per_second': -1, 'max_peak_rss_bytes': -1,
                     'samples': [{'requests_per_second': rps, 'latency_p99_ns': p99,
                                  'requests': 10, 'server_cpu_ns': server,
                                  'client_cpu_ns': client, 'process_cpu_ns': server + client,
                                  'peak_rss_bytes': rss}]})
    result = bench_pair.aggregate(rows)
    self.assertEqual({row['transport'] for row in result}, {'bemo', 'epoll'})
    for row in result:
      self.assertEqual(len(row['samples']), 3)
      self.assertEqual(row['median_requests_per_second'], 200)
      self.assertEqual(row['median_latency_p99_ns'], 20)
      self.assertEqual(row['median_server_cpu_ns_per_request'], 2)
      self.assertEqual(row['median_client_cpu_ns_per_request'], 2)
      self.assertEqual(row['median_cpu_ns_per_request'], 4)
      self.assertEqual(row['max_peak_rss_bytes'], 300)
    self.assertTrue(all(len(row['samples']) == 1 for row in rows), 'inputs must remain intact')


if __name__ == '__main__':
  unittest.main()
