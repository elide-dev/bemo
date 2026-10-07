"""Validate ownership benchmark values and reject incomplete output."""
import unittest

import bench_leases


class LeaseMeasurementTest(unittest.TestCase):
  def test_units_and_complete_cases(self):
    log = '''buffer/retain-drop/4096 time: [1.0 ns 2.0 ns 3.0 ns]
buffer/slice-drop/4096 time: [0.001 us 0.002 us 0.003 us]
buffer/exclusive-freeze-recover
  time: [1e-3 µs 2e-3 µs 3e-3 µs]'''
    values = bench_leases.measurements(log)
    self.assertEqual(len(values), 3)
    self.assertTrue(all(value['point_ns'] == 2 for value in values))
    with self.assertRaisesRegex(ValueError, 'Incomplete'):
      bench_leases.measurements(log.split('buffer/exclusive')[0])


if __name__ == '__main__':
  unittest.main()
