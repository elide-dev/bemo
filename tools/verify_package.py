#!/usr/bin/env python3
"""Validate Maven metadata, checksums, isolation, and FFM from the packaged JARs."""
import hashlib
import os
import platform
from pathlib import Path
import tempfile
import xml.etree.ElementTree as ET
import zipfile

from build import MAVEN_GROUP, MAVEN_PATH, BUILD, MODULES, ROOT, VERSION, classifier, classpath, compile_java, java_tool, run, netty


def verify():
  stage = BUILD / "maven" / MAVEN_PATH
  ns = {"m": "http://maven.apache.org/POM/4.0.0"}
  jars = {}
  for module in MODULES:
    name = f"bemo-{module}"
    prefix = stage / name / VERSION / f"{name}-{VERSION}"
    for suffix in (".pom", ".jar", "-sources.jar", "-javadoc.jar"):
      path = Path(f"{prefix}{suffix}")
      data = path.read_bytes()
      for algorithm in ("md5", "sha1", "sha256", "sha512"):
        expected = Path(f"{path}.{algorithm}").read_text().strip()
        assert hashlib.new(algorithm, data).hexdigest() == expected, path
    pom = ET.parse(f"{prefix}.pom")
    assert pom.findtext("m:groupId", namespaces=ns) == MAVEN_GROUP
    for dep in pom.findall("m:dependencies/m:dependency", ns):
      if dep.findtext("m:artifactId", namespaces=ns).startswith("bemo-"):
        assert dep.findtext("m:groupId", namespaces=ns) == MAVEN_GROUP
    assert pom.findtext("m:artifactId", namespaces=ns) == name
    assert pom.findtext("m:version", namespaces=ns) == VERSION
    for field in ("licenses", "developers", "scm", "description", "url"):
      assert pom.find(f"m:{field}", ns) is not None, field
    with zipfile.ZipFile(f"{prefix}.jar") as jar:
      names = jar.namelist()
      assert "META-INF/LICENSE" in names and "META-INF/NOTICE" in names
      assert any(n.endswith(".class") for n in names)
      assert not any(n.startswith(("org/graalvm/", "dev/elide/runtime/")) for n in names)
      netty_classes = [n for n in names if n.startswith("io/netty/") and n.endswith(".class")]
      assert netty_classes == (["io/netty/handler/ssl/ApplicationProtocolSslEngine.class"] if module == "netty" else [])
      for entry in names:
        if entry.endswith(".class"):
          assert entry.startswith("dev/elide/bemo/") or entry == "io/netty/handler/ssl/ApplicationProtocolSslEngine.class", \
              f"Unexpected published Java namespace: {entry}"
          assert int.from_bytes(jar.read(entry)[6:8], "big") <= 66, "JDK 22 baseline exceeded"
      if module == "netty":
        metadata = "META-INF/native-image/dev.elide.bemo/bemo-netty/"
        assert metadata + "reflect-config.json" in names
        assert metadata + "native-image.properties" in names
    jars[module] = Path(f"{prefix}.jar")
  for module in ("ffm", "native-image"):
    native_jar = stage / f"bemo-{module}" / VERSION / f"bemo-{module}-{VERSION}-{classifier()}.jar"
    assert native_jar.is_file()
    with zipfile.ZipFile(native_jar) as archive:
      resource = f"META-INF/native/{classifier()}/"
      assert resource + "bemo.h" in archive.namelist()
      assert resource + "elide_transport.h" in archive.namelist()
      for algorithm in ("md5", "sha1", "sha256", "sha512"):
        assert hashlib.new(algorithm, native_jar.read_bytes()).hexdigest() == Path(f"{native_jar}.{algorithm}").read_text().strip()
      if module == "native-image":
        assert any(name.endswith((".a", ".lib")) for name in archive.namelist())
  ffm_jar = stage / "bemo-ffm" / VERSION / f"bemo-ffm-{VERSION}-{classifier()}.jar"
  with tempfile.TemporaryDirectory(prefix="bemo-package-") as tmp:
    with zipfile.ZipFile(ffm_jar) as archive:
      entries = [n for n in archive.namelist() if n.endswith((".dylib", ".so", ".dll"))]
      assert len(entries) == 1
      expected_library = {"Darwin": "libbemo_ffi.dylib", "Linux": "libbemo_ffi.so", "Windows": "bemo_ffi.dll"}[platform.system()]
      assert Path(entries[0]).name == expected_library, "Unexpected published native library name"
      binary = Path(tmp) / Path(entries[0]).name
      binary.write_bytes(archive.read(entries[0]))
    cp = [jars["api"], jars["ffm"]]
    tests = ROOT / "tests/java/dev/elide/bemo"
    output = BUILD / "tests/package"
    compile_java(output, [tests / "Contract.java", tests / "FfmContract.java"], cp)
    run(os.environ.get("BEMO_TEST_JAVA", java_tool("java")), "--enable-native-access=ALL-UNNAMED",
        "-ea", "-cp", classpath([output, *cp]), "dev.elide.bemo.FfmContract", binary, timeout=60)
    transport_cp = [jars["api"], jars["ffm"], jars["netty"], *netty()]
    transport_sources = [p for p in (ROOT / "tests/transport/java").glob("*.java") if not p.name.startswith("Capi")]
    transport_output = BUILD / "tests/package-transport"
    compile_java(transport_output, sorted(transport_sources), transport_cp,
                 lint="all,-restricted,-deprecation,-try,-serial")
    fixtures = ROOT / "crates/bemo/tests/fixtures"
    for contract in ("StandaloneTransportCheck", "NativeTlsChannelTest"):
      run(os.environ.get("BEMO_TEST_JAVA", java_tool("java")), "--enable-native-access=ALL-UNNAMED",
          "-ea", "-cp", classpath([transport_output, *transport_cp]), contract, binary,
          fixtures / "localhost-cert.pem", fixtures / "localhost-key.pem", timeout=90)
  # Exercise auto-loading from the classifier JAR itself, with no pre-extracted path.
  loader_output = BUILD / "tests/package-loader"
  cp = [jars["api"], jars["ffm"]]
  tests = ROOT / "tests/java/dev/elide/bemo"
  compile_java(loader_output, [tests / "Contract.java", tests / "ffm/NativeLibraryLoaderContract.java"], cp)
  java = os.environ.get("BEMO_TEST_JAVA", java_tool("java"))
  with tempfile.TemporaryDirectory(prefix="bemo loader ") as tmp:
    workdir = Path(tmp) / "native files"
    def loader_check(extra=(), properties=(), expected=()):
      run(java, "--enable-native-access=ALL-UNNAMED", *properties, "-ea", "-cp",
          classpath([loader_output, *cp, *extra]), "dev.elide.bemo.ffm.NativeLibraryLoaderContract",
          *expected, timeout=60)
    loader_check(expected=["Missing META-INF/native/"])
    loader_check([ffm_jar], [f"-Dbemo.native.workdir={workdir}"])
    assert not list(workdir.iterdir()), "Extracted library was not removed at JVM exit"
    duplicate = Path(tmp) / "duplicate.jar"
    duplicate.write_bytes(ffm_jar.read_bytes())
    loader_check([ffm_jar, duplicate])
    with zipfile.ZipFile(duplicate, "w") as archive:
      with zipfile.ZipFile(ffm_jar) as original:
        entry = next(n for n in original.namelist() if n.endswith((".so", ".dll", ".dylib")))
      archive.writestr(entry, b"conflicting resource")
    loader_check([ffm_jar, duplicate], expected=["Conflicting Bemo native resources"])
    loader_check(properties=["-Dbemo.native.path=relative-library"], expected=["existing absolute library path"])
    # The explicit override wins even when no classifier is present.
    with zipfile.ZipFile(ffm_jar) as archive:
      override = Path(tmp) / Path(entry).name
      override.write_bytes(archive.read(entry))
    loader_check(properties=[f"-Dbemo.native.path={override}"])
    run(java, "--enable-native-access=ALL-UNNAMED", "-ea", "-cp",
        classpath([transport_output, *transport_cp, ffm_jar]), "AutomaticTransportCheck",
        fixtures / "localhost-cert.pem", fixtures / "localhost-key.pem", timeout=90)
    # Link a consumer against the actual published static classifier and its headers.
    static_jar = stage / "bemo-native-image" / VERSION / f"bemo-native-image-{VERSION}-{classifier()}.jar"
    static_dir = Path(tmp) / "static"
    with zipfile.ZipFile(static_jar) as archive:
      resource = f"META-INF/native/{classifier()}/"
      static_dir.mkdir()
      for entry in archive.namelist():
        if entry.startswith(resource) and not entry.endswith("/"):
          (static_dir / Path(entry).name).write_bytes(archive.read(entry))
    if os.name != "nt":
      binary = Path(tmp) / "static-contract"
      flags = ["-ldl", "-lpthread", "-lm"] if platform.system() == "Linux" else []
      run(os.environ.get("CC", "cc"), "-std=c11", "-Wall", "-Wextra", "-Werror", "-I", static_dir,
          ROOT / "tests/static.c", static_dir / "libbemo_ffi.a", *flags, "-o", binary)
      run(binary, timeout=30)
  print("Maven package contracts passed (resource loading and static linkage)")


if __name__ == "__main__":
  verify()
