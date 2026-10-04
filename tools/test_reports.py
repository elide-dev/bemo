import sys
import tempfile
import unittest
from pathlib import Path
import xml.etree.ElementTree as ET

from reports import Reports


class ReportTests(unittest.TestCase):
  def test_failure_timeout_and_xml_escaping(self):
    with tempfile.TemporaryDirectory() as directory:
      reports = Reports(directory)
      reports.run("ok", [sys.executable, "-c", "print('<hello>\\x01')"], timeout=5, cwd=directory)
      reports.run("failure", [sys.executable, "-c", "raise RuntimeError('broken')"], timeout=5, cwd=directory)
      reports.run("timeout", [sys.executable, "-c", "import time; time.sleep(5)"], timeout=0.1, cwd=directory)
      for name, failures in (("ok", "0"), ("failure", "1"), ("timeout", "1")):
        suite = ET.parse(Path(directory) / f"TEST-{name}.xml").getroot()
        self.assertEqual(suite.get("failures"), failures)
        self.assertEqual(suite.get("tests"), "1")
      with self.assertRaises(RuntimeError):
        reports.finish()
      self.assertEqual(len(ET.parse(Path(directory) / "junit.xml").getroot()), 3)
      Reports(directory)
      self.assertFalse(list(Path(directory).glob("TEST-*.xml")))


if __name__ == "__main__":
  unittest.main()
