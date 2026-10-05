"""Check native library deployment requirements against the qualified builders."""
import platform
import re
import subprocess


def version(value):
  parts = tuple(map(int, value.split(".")))
  return (parts + (0, 0, 0))[:3]


def compatibility(binary):
  system = platform.system()
  if system == "Linux":
    output = subprocess.check_output(["readelf", "--version-info", str(binary)], text=True)
    required = sorted(set(re.findall(r"GLIBC_(\d+\.\d+(?:\.\d+)?)", output)), key=version)
    floor = "2.39"
    if not required:
      raise RuntimeError("No glibc version requirements found")
    if version(required[-1]) > version(floor):
      raise RuntimeError(f"Library needs glibc {required[-1]}, above qualified floor {floor}")
    return {"os": "linux", "glibc_floor": floor, "max_glibc_required": required[-1]}
  if system == "Darwin":
    output = subprocess.check_output(["otool", "-l", str(binary)], text=True)
    requirements = re.findall(r"\bminos\s+(\d+(?:\.\d+){0,2})", output)
    requirements += re.findall(r"cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version (\d+(?:\.\d+){0,2})", output)
    floor = "15.0"
    if not requirements:
      raise RuntimeError("No macOS deployment requirement found")
    required = max(requirements, key=version)
    if version(required) > version(floor):
      raise RuntimeError(f"Library needs macOS {required}, above qualified floor {floor}")
    return {"os": "macos", "macos_floor": floor, "deployment_target": required}
  raise RuntimeError("Release compatibility is qualified only for glibc Linux and macOS")
