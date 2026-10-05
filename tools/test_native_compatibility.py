import unittest
from unittest.mock import patch

from native_compatibility import compatibility


class CompatibilityTests(unittest.TestCase):
  def test_glibc_floor_rejects_newer_symbols(self):
    with patch("native_compatibility.platform.system", return_value="Linux"), \
         patch("native_compatibility.subprocess.check_output", return_value="GLIBC_2.17 GLIBC_2.39"):
      self.assertEqual(compatibility("library")["max_glibc_required"], "2.39")
    with patch("native_compatibility.platform.system", return_value="Linux"), \
         patch("native_compatibility.subprocess.check_output", return_value="GLIBC_2.40"):
      with self.assertRaisesRegex(RuntimeError, "above qualified"):
        compatibility("library")

  def test_macos_floor_checks_deployment_command(self):
    with patch("native_compatibility.platform.system", return_value="Darwin"), \
         patch("native_compatibility.subprocess.check_output", return_value="cmd LC_BUILD_VERSION\nminos 15.0\nsdk 27.0"):
      self.assertEqual(compatibility("library")["deployment_target"], "15.0")
    with patch("native_compatibility.platform.system", return_value="Darwin"), \
         patch("native_compatibility.subprocess.check_output", return_value="cmd LC_BUILD_VERSION\nminos 26.0"):
      with self.assertRaisesRegex(RuntimeError, "above qualified"):
        compatibility("library")

  def test_missing_requirements_fail_instead_of_claiming_compatibility(self):
    for system in ("Linux", "Darwin"):
      with self.subTest(system=system), patch("native_compatibility.platform.system", return_value=system), \
           patch("native_compatibility.subprocess.check_output", return_value=""):
        with self.assertRaisesRegex(RuntimeError, "No .* requirement"):
          compatibility("library")
