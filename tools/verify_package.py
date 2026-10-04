#!/usr/bin/env python3
"""Validate Maven metadata, checksums, isolation, and FFM from the packaged JARs."""
import hashlib
import os
from pathlib import Path
import tempfile
import xml.etree.ElementTree as ET
import zipfile

from build import BUILD, MODULES, ROOT, VERSION, classifier, classpath, compile_java, java_tool, run


def verify():
  stage = BUILD / "maven/dev/elide"
  ns = {"m": "http://maven.apache.org/POM/4.0.0"}
  jars = {}
  for module in MODULES:
    name = f"dokar-{module}"
    prefix = stage / name / VERSION / f"{name}-{VERSION}"
    for suffix in (".pom", ".jar", "-sources.jar", "-javadoc.jar"):
      path = Path(f"{prefix}{suffix}")
      data = path.read_bytes()
      for algorithm in ("md5", "sha1", "sha256", "sha512"):
        expected = Path(f"{path}.{algorithm}").read_text().strip()
        assert hashlib.new(algorithm, data).hexdigest() == expected, path
    pom = ET.parse(f"{prefix}.pom")
    assert pom.findtext("m:artifactId", namespaces=ns) == name
    assert pom.findtext("m:version", namespaces=ns) == VERSION
    for field in ("licenses", "developers", "scm", "description", "url"):
      assert pom.find(f"m:{field}", ns) is not None, field
    with zipfile.ZipFile(f"{prefix}.jar") as jar:
      names = jar.namelist()
      assert "META-INF/LICENSE" in names and "META-INF/NOTICE" in names
      assert any(n.endswith(".class") for n in names)
      assert not any(n.startswith(("org/graalvm/", "dev/elide/runtime/", "io/netty/")) for n in names)
      for entry in names:
        if entry.endswith(".class"):
          assert int.from_bytes(jar.read(entry)[6:8], "big") <= 66, "JDK 22 baseline exceeded"
    jars[module] = Path(f"{prefix}.jar")
  for module in ("ffm", "native-image"):
    native_jar = stage / f"dokar-{module}" / VERSION / f"dokar-{module}-{VERSION}-{classifier()}.jar"
    assert native_jar.is_file()
    with zipfile.ZipFile(native_jar) as archive:
      assert any(name.endswith("dokar.h") for name in archive.namelist())
      if module == "native-image":
        assert any(name.endswith((".a", ".lib")) for name in archive.namelist())
  ffm_jar = stage / "dokar-ffm" / VERSION / f"dokar-ffm-{VERSION}-{classifier()}.jar"
  with tempfile.TemporaryDirectory(prefix="dokar-package-") as tmp:
    with zipfile.ZipFile(ffm_jar) as archive:
      entries = [n for n in archive.namelist() if n.endswith((".dylib", ".so", ".dll"))]
      assert len(entries) == 1
      binary = Path(tmp) / Path(entries[0]).name
      binary.write_bytes(archive.read(entries[0]))
    cp = [jars["api"], jars["ffm"]]
    tests = ROOT / "tests/java/dev/elide/dokar"
    output = BUILD / "tests/package"
    compile_java(output, [tests / "Contract.java", tests / "FfmContract.java"], cp)
    run(os.environ.get("DOKAR_TEST_JAVA", java_tool("java")), "--enable-native-access=ALL-UNNAMED",
        "-ea", "-cp", classpath([output, *cp]), "dev.elide.dokar.FfmContract", binary, timeout=60)
  print("Maven package contracts passed")


if __name__ == "__main__":
  verify()
