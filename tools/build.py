#!/usr/bin/env python3
"""Build, check, test, and stage Dokar with Cargo and Elide (Python 3.11+)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / "build"
VERSIONS = json.loads((ROOT / "tools/versions.json").read_text())
VERSION = (ROOT / ".version").read_text().strip()
ELIDE = os.environ.get("ELIDE", "elide")
MODULES = ("api", "ffm", "native-image", "netty")
REPOSITORY = "https://github.com/elide-dev/dokar"


def run(*args, **kwargs):
  print("+", " ".join(map(str, args)), flush=True)
  subprocess.run(list(map(str, args)), cwd=ROOT, check=True, **kwargs)


def jar_dependency(group, artifact, version, classifier=""):
  suffix = f"-{classifier}" if classifier else ""
  path = ROOT / ".dev/dependencies/m2" / group.replace(".", "/") / artifact / version
  path /= f"{artifact}-{version}{suffix}.jar"
  if not path.is_file():
    raise RuntimeError(f"Missing pinned dependency {path}; run make deps")
  return path


def sdk():
  return [jar_dependency("org.graalvm.sdk", name, VERSIONS["graalvm_sdk"])
          for name in ("nativeimage", "word")]


def netty():
  return [jar_dependency("io.netty", name, VERSIONS["netty"]) for name in (
      "netty-common", "netty-buffer", "netty-transport", "netty-resolver", "netty-handler",
      "netty-codec-base", "netty-codec-compression", "netty-codec-http", "netty-codec-http2",
      "netty-transport-native-unix-common")]


def classpath(paths):
  return os.pathsep.join(map(str, paths))


def sources(module):
  return sorted((ROOT / "packages" / module / "src/main/java").rglob("*.java"))


def classes(module):
  return BUILD / "classes" / module


def deps():
  run(ELIDE, "install", "--slim", "--direct")


def rust(release=False):
  run("cargo", "build", "--workspace", "--locked", *(["--release"] if release else []))


def target_dir(release=False):
  metadata = subprocess.check_output(
      ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=ROOT, text=True)
  return Path(json.loads(metadata)["target_directory"]) / ("release" if release else "debug")


def library(release=False):
  name = {"Darwin": "libdokar_ffi.dylib", "Linux": "libdokar_ffi.so", "Windows": "dokar_ffi.dll"}
  return target_dir(release) / name[platform.system()]


def compile_java(output, inputs, dependencies=(), lint="all"):
  shutil.rmtree(output, ignore_errors=True)
  output.mkdir(parents=True)
  run(ELIDE, "javac", "--", "--release", VERSIONS["jvm_release"], f"-Xlint:{lint}", "-Werror",
      "-cp", classpath(dependencies) or str(output), "-d", output, *inputs)


def jvm():
  deps()
  for module in MODULES:
    cp = [] if module == "api" else [classes("api")]
    if module == "native-image":
      cp += sdk()
    if module == "netty":
      cp += netty()
    compile_java(classes(module), sources(module), cp)
    resources = ROOT / "packages" / module / "src/main/resources"
    if resources.is_dir():
      shutil.copytree(resources, classes(module), dirs_exist_ok=True)


def java_tool(name):
  home = os.environ.get("JAVA_HOME")
  if home:
    suffix = ".exe" if os.name == "nt" else ""
    tool = Path(home) / "bin" / (name + suffix)
    if tool.is_file():
      return tool
  found = shutil.which(name)
  if not found:
    raise RuntimeError(f"{name} is required; configure JAVA_HOME or PATH")
  return found


def test_jvm():
  rust()
  jvm()
  test_root = ROOT / "tests/java/dev/elide/dokar"
  output = BUILD / "tests/ffm"
  cp = [classes("api"), classes("ffm")]
  compile_java(output, [test_root / "Contract.java", test_root / "FfmContract.java"], cp)
  extra = []
  if os.name != "nt":
    incompatible = BUILD / "tests" / library().name.replace("dokar_ffi", "incompatible")
    kind = "-dynamiclib" if platform.system() == "Darwin" else "-shared"
    run(os.environ.get("CC", "cc"), kind, "-fPIC", ROOT / "tests/incompatible.c", "-o", incompatible)
    extra.append(incompatible)
  # Deliberately launch stock java, with no Elide or GraalVM SDK in the classpath.
  run(os.environ.get("DOKAR_TEST_JAVA", java_tool("java")), "--enable-native-access=ALL-UNNAMED", "-ea", "-cp",
      classpath([output, *cp]), "dev.elide.dokar.FfmContract", library(), *extra, timeout=60)
  if os.name != "nt":
    binary = BUILD / "tests/abi"
    run(os.environ.get("CC", "cc"), "-std=c11", "-Wall", "-Wextra", "-Werror",
        "-I", ROOT / "include", ROOT / "tests/abi.c", library(), "-o", binary)
    run(binary, timeout=30)
  test_transport()


def test_native_image():
  rust()
  jvm()
  test_root = ROOT / "tests/java/dev/elide/dokar"
  output = BUILD / "tests/capi"
  cp = [classes("api"), classes("native-image"), *sdk()]
  compile_java(output, [test_root / "Contract.java", test_root / "CapiContract.java"], cp)
  binary = BUILD / "tests" / ("capi-contract.exe" if os.name == "nt" else "capi-contract")
  linker = {
      "Linux": ["-H:NativeLinkerOption=-ldl", "-H:NativeLinkerOption=-lpthread", "-H:NativeLinkerOption=-lm"],
      "Darwin": [],
      "Windows": ["-H:NativeLinkerOption=ntdll.lib"],
  }[platform.system()]
  run(java_tool("native-image"), "--no-fallback", "-O0", "-cp", classpath([output, *cp]),
      f"-H:CLibraryPath={target_dir()}", f"--native-compiler-options=-I{ROOT / 'include'}",
      *linker, "dev.elide.dokar.CapiContract", binary, timeout=900)
  run(binary, timeout=60)
  test_transport(native_image=True)


def test_transport(native_image=False):
  output = BUILD / "tests/transport"
  cp = [classes("api"), classes("ffm"), classes("netty"), *netty()]
  compile_java(output, sorted((ROOT / "tests/transport/java").glob("*.java")),
               [*cp, classes("native-image"), *sdk()], lint="all,-restricted,-deprecation,-try,-serial")
  fixtures = ROOT / "crates/dokar/tests/fixtures"
  cert, key = fixtures / "localhost-cert.pem", fixtures / "localhost-key.pem"
  if native_image:
    binary = BUILD / "tests" / ("transport-capi.exe" if os.name == "nt" else "transport-capi")
    linker = ["-H:NativeLinkerOption=ntdll.lib"] if os.name == "nt" else []
    run(java_tool("native-image"), "--no-fallback", "--enable-monitoring=jfr", "-O0",
        "-cp", classpath([output, *cp, classes("native-image"), *sdk()]),
        f"-H:CLibraryPath={target_dir()}", f"--native-compiler-options=-I{ROOT / 'include'}",
        *linker, "CapiTlsChannelTest", binary, timeout=900)
    run(binary, cert, key, timeout=180)
  else:
    for contract in ("FfmTransportTest", "NativeByteBufTest", "NativeChannelTest", "NativeLifecycleTest",
                     "NativeTlsChannelTest", "NativeTlsNegativeTest", "NativeTlsOrderingTest",
                     "NativeTransferTest", "NativeJfrTest", "NativeReentrantCloseTest",
                     "NativeReceiveAllocatorTest", "NativeSslEngineTest", "NativeSslInteropTest",
                     "NativeSslPolicyTest", "StandaloneTransportCheck"):
      run(os.environ.get("DOKAR_TEST_JAVA", java_tool("java")), "--enable-native-access=ALL-UNNAMED",
          "-ea", "-cp", classpath([output, *cp]), contract, library(), cert, key, timeout=90)


def fmt(check=False):
  deps()
  run("cargo", "fmt", "--package", "dokar", "--package", "dokar-ffi", *(["--check"] if check else []))
  formatter = jar_dependency("com.google.googlejavaformat", "google-java-format",
                             VERSIONS["java_format"], "all-deps")
  java_files = [p for module in MODULES for p in sources(module)] + sorted((ROOT / "tests").rglob("*.java"))
  flags = ["--dry-run", "--set-exit-if-changed"] if check else ["--replace"]
  run(ELIDE, "java", "--", "-jar", formatter, *flags, *java_files)


def check():
  run(sys.executable, ROOT / "tools/generate_exports.py", "--check")
  fmt(True)
  run("cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")
  run("cargo", "doc", "--workspace", "--no-deps", "--locked", env={**os.environ, "RUSTDOCFLAGS": "-D warnings"})
  cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
  if VERSION.removesuffix("-SNAPSHOT") != cargo["workspace"]["package"]["version"]:
    raise RuntimeError("Cargo.toml and .version disagree")
  if (ROOT / ".elide-version").read_text().strip() != VERSIONS["elide"]:
    raise RuntimeError("Elide version pins disagree")
  jvm()


def classifier():
  os_name = {"Darwin": "osx", "Linux": "linux", "Windows": "windows"}[platform.system()]
  arch = {"arm64": "aarch64", "aarch64": "aarch64", "AMD64": "x86_64", "x86_64": "x86_64"}[platform.machine()]
  libc = ""
  if os_name == "linux":
    family = platform.libc_ver()[0]
    if family != "glibc":
      raise RuntimeError("Native packaging currently validates glibc Linux only")
    libc = "-gnu"
  return f"{os_name}-{arch}{libc}"


def pom(path, artifact, dependencies):
  ns = "http://maven.apache.org/POM/4.0.0"
  ET.register_namespace("", ns)
  project = ET.Element(f"{{{ns}}}project")
  def add(parent, name, value=None):
    node = ET.SubElement(parent, f"{{{ns}}}{name}")
    node.text = value
    return node
  for name, value in (("modelVersion", "4.0.0"), ("groupId", "dev.elide"), ("artifactId", artifact),
                      ("version", VERSION), ("packaging", "jar"), ("name", artifact),
                      ("description", "Dokar native transport for Netty, JVM FFM, and Native Image"),
                      ("url", REPOSITORY)):
    add(project, name, value)
  license_node = add(add(project, "licenses"), "license")
  add(license_node, "name", "Apache License, Version 2.0")
  add(license_node, "url", "https://www.apache.org/licenses/LICENSE-2.0.txt")
  developer = add(add(project, "developers"), "developer")
  add(developer, "id", "elide")
  add(developer, "name", "Elide Technologies, Inc.")
  add(developer, "url", "https://elide.dev")
  scm = add(project, "scm")
  add(scm, "url", REPOSITORY)
  add(scm, "connection", f"scm:git:{REPOSITORY}.git")
  add(scm, "developerConnection", "scm:git:ssh://git@github.com/elide-dev/dokar.git")
  if dependencies:
    deps_node = add(project, "dependencies")
    for group, name, version, scope in dependencies:
      dep = add(deps_node, "dependency")
      for key, value in (("groupId", group), ("artifactId", name), ("version", version), ("scope", scope)):
        add(dep, key, value)
  ET.indent(project, space="  ")
  ET.ElementTree(project).write(path, encoding="utf-8", xml_declaration=True)


def jar(path, directory):
  run(ELIDE, "jar", "--", "--create", "--file", path,
      "--date=2026-01-01T00:00:00Z", "-C", directory, ".")


def package():
  rust(True)
  jvm()
  stage = BUILD / "maven"
  shutil.rmtree(stage, ignore_errors=True)
  for module in MODULES:
    artifact = f"dokar-{module}"
    destination = stage / "dev/elide" / artifact / VERSION
    destination.mkdir(parents=True)
    prefix = destination / f"{artifact}-{VERSION}"
    metadata = classes(module) / "META-INF"
    metadata.mkdir(exist_ok=True)
    for name in ("LICENSE", "NOTICE"):
      shutil.copy2(ROOT / name, metadata / name)
    jar(f"{prefix}.jar", classes(module))
    jar(f"{prefix}-sources.jar", ROOT / "packages" / module / "src/main/java")
    docs = BUILD / "javadoc" / module
    shutil.rmtree(docs, ignore_errors=True)
    cp = [] if module == "api" else [classes("api")]
    if module == "native-image":
      cp += sdk()
    if module == "netty":
      cp += netty()
    # Javadoc is run by Elide's JVM toolchain, not a second build system.
    run(ELIDE, "java", "--", "-m", "jdk.javadoc/jdk.javadoc.internal.tool.Main", "-quiet",
        "-notimestamp", "-Werror", "-Xdoclint:all,-missing", "--release", VERSIONS["jvm_release"], "-d", docs,
        "-classpath", classpath(cp) or str(classes(module)), *sources(module))
    jar(f"{prefix}-javadoc.jar", docs)
    dependencies = [] if module == "api" else [("dev.elide", "dokar-api", VERSION, "compile")]
    if module == "native-image":
      dependencies += [("org.graalvm.sdk", name, VERSIONS["graalvm_sdk"], "provided")
                       for name in ("nativeimage", "word")]
    if module == "netty":
      dependencies += [("io.netty", name, VERSIONS["netty"], "compile") for name in (
          "netty-transport", "netty-handler", "netty-codec-http2", "netty-transport-native-unix-common")]
    pom(f"{prefix}.pom", artifact, dependencies)
  # Attach separate dynamic and static native classifiers; keep base JARs portable.
  static_name = "dokar_ffi.lib" if os.name == "nt" else "libdokar_ffi.a"
  for module, binary in (("ffm", library(True)), ("native-image", target_dir(True) / static_name)):
    native = BUILD / "native-resources" / module
    shutil.rmtree(native, ignore_errors=True)
    resource = native / "META-INF/native" / classifier()
    resource.mkdir(parents=True)
    shutil.copy2(binary, resource)
    shutil.copy2(ROOT / "include/dokar.h", resource)
    shutil.copy2(ROOT / "include/elide_transport.h", resource)
    for name in ("LICENSE", "NOTICE"):
      shutil.copy2(ROOT / name, native / "META-INF" / name)
    prefix = stage / "dev/elide" / f"dokar-{module}" / VERSION / f"dokar-{module}-{VERSION}"
    jar(f"{prefix}-{classifier()}.jar", native)
  for path in sorted(stage.rglob("*")):
    if path.is_file():
      for algorithm in ("md5", "sha1", "sha256", "sha512"):
        digest = hashlib.new(algorithm, path.read_bytes()).hexdigest()
        Path(f"{path}.{algorithm}").write_text(digest + "\n")
  bundle = BUILD / f"dokar-{VERSION}-{classifier()}-unsigned.zip"
  with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED) as archive:
    for path in sorted(stage.rglob("*")):
      if path.is_file():
        archive.write(path, path.relative_to(stage))
  print(f"Staged unsigned Maven repository: {stage}\nBundle: {bundle}")


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("deps", "build", "jvm", "test", "test-jvm", "test-native-image",
                                       "check", "fmt", "fmt-check", "package", "clean"))
  task = parser.parse_args().task
  if task == "deps": deps()
  elif task == "build": rust(); jvm()
  elif task == "jvm": jvm()
  elif task == "test":
    run("cargo", "test", "--workspace", "--all-targets", "--locked")
    run("cargo", "test", "--workspace", "--doc", "--locked")
    run(sys.executable, ROOT / "tools/test_git_dependency.py")
    test_jvm()
  elif task == "test-jvm": test_jvm()
  elif task == "test-native-image": test_native_image()
  elif task == "check": check()
  elif task == "fmt": fmt()
  elif task == "fmt-check": fmt(True)
  elif task == "package": package()
  elif task == "clean":
    shutil.rmtree(BUILD, ignore_errors=True)
    run("cargo", "clean")


if __name__ == "__main__":
  try:
    main()
  except (subprocess.CalledProcessError, subprocess.TimeoutExpired, RuntimeError, OSError) as failure:
    print(f"error: {failure}", file=sys.stderr)
    sys.exit(1)
