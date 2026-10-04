"""JUnit reports for process-isolated JVM and C ABI contracts."""
import re
import subprocess
import time
from pathlib import Path
import xml.etree.ElementTree as ET


def xml_text(value):
  return re.sub(r"[^\x09\x0a\x0d\x20-\ud7ff\ue000-\ufffd\U00010000-\U0010ffff]", "", value)


class Reports:
  def __init__(self, directory):
    self.directory = Path(directory)
    self.directory.mkdir(parents=True, exist_ok=True)
    for old in [*self.directory.glob("TEST-*.xml"), *self.directory.glob("*.log")]:
      old.unlink()
    (self.directory / "junit.xml").unlink(missing_ok=True)
    self.failures = []

  def run(self, name, command, *, timeout, cwd):
    started = time.monotonic()
    error = None
    log = self.directory / f"{name}.log"
    with log.open("w") as output:
      try:
        subprocess.run(list(map(str, command)), cwd=cwd, stdout=output,
                       stderr=subprocess.STDOUT, check=True, timeout=timeout)
      except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as failure:
        error = str(failure)
    elapsed = time.monotonic() - started
    text = xml_text(log.read_text(errors="replace"))
    print(text, end="", flush=True)
    suite = ET.Element("testsuite", name=name, tests="1", failures=str(int(error is not None)),
                       errors="0", skipped="0", time=f"{elapsed:.6f}")
    case = ET.SubElement(suite, "testcase", classname=name, name="contract", time=f"{elapsed:.6f}")
    if error:
      ET.SubElement(case, "failure", message=xml_text(error)).text = text
      self.failures.append(name)
    ET.SubElement(case, "system-out").text = text
    ET.ElementTree(suite).write(self.directory / f"TEST-{name}.xml", encoding="utf-8", xml_declaration=True)

  def finish(self):
    suites = ET.Element("testsuites")
    for path in sorted(self.directory.glob("TEST-*.xml")):
      suites.append(ET.parse(path).getroot())
    ET.ElementTree(suites).write(self.directory / "junit.xml", encoding="utf-8", xml_declaration=True)
    if self.failures:
      raise RuntimeError("Failed contracts: " + ", ".join(self.failures))
