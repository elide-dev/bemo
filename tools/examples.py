#!/usr/bin/env python3
"""Stage and smoke-test the framework examples using the repository's Elide toolchain."""
import argparse
import concurrent.futures
import gzip
import http.client
import os
import platform
import shutil
import socket
import ssl
import subprocess
import time

import build

PROJECTS = {"spring-boot": "spring", "micronaut": "micronaut"}
STAGE = build.BUILD / "examples"


def prepare():
  """Stage optimized native artifacts and the Java bindings; never publish."""
  build.rust(release=True)
  # Bemo's version is stable across checkout edits, so invalidate only our staged
  # group's cache before Maven can reuse an older build of the same coordinates.
  shutil.rmtree(STAGE / "m2" / build.MAVEN_PATH, ignore_errors=True)
  for module in build.MODULES:
    artifact = f"bemo-{module}"
    destination = STAGE / "maven" / build.MAVEN_PATH / artifact / build.VERSION
    destination.mkdir(parents=True, exist_ok=True)
    prefix = destination / f"{artifact}-{build.VERSION}"
    build.jar(f"{prefix}.jar", build.classes(module))
    shutil.copy2(f"{prefix}.jar", STAGE / f"{artifact}.jar")
    dependencies = [("org.jspecify", "jspecify", build.VERSIONS["jspecify"], "compile")] if module == "api" else [(build.MAVEN_GROUP, "bemo-api", build.VERSION, "compile")]
    if module == "native-image":
      dependencies += [("org.graalvm.sdk", name, build.VERSIONS["graalvm_sdk"], "provided")
                       for name in ("nativeimage", "word")]
    if module == "netty":
      dependencies += [("io.netty", name, build.VERSIONS["netty"], "compile") for name in (
          "netty-transport", "netty-handler", "netty-codec-http2", "netty-transport-native-unix-common")]
    build.pom(f"{prefix}.pom", artifact, dependencies)
  static = STAGE / "static"
  static.mkdir(parents=True, exist_ok=True)
  archive = "bemo_ffi.lib" if os.name == "nt" else "libbemo_ffi.a"
  shutil.copy2(build.target_dir(release=True) / archive, static)
  shutil.copytree(build.ROOT / "include", STAGE / "include", dirs_exist_ok=True)
  resources = STAGE / "native-resources"
  shutil.rmtree(resources, ignore_errors=True)
  destination = resources / "META-INF/native" / build.classifier()
  destination.mkdir(parents=True)
  shutil.copy2(build.library(release=True), destination)
  native_jar = STAGE / "bemo-native.jar"
  build.jar(native_jar, resources)
  build.jar(STAGE / "bemo-example-resources.jar", build.ROOT / "examples/shared/src/main/resources")
  destination = STAGE / "maven" / build.MAVEN_PATH / "bemo-ffm" / build.VERSION
  shutil.copy2(native_jar, destination / f"bemo-ffm-{build.VERSION}-{build.classifier()}.jar")


def compile_projects(builder="elide", projects=None):
  for project in projects or PROJECTS:
    if builder == "gradle":
      directory = build.ROOT / "examples" / project
      wrapper = directory / ("gradlew.bat" if os.name == "nt" else "gradlew")
      build.run(wrapper, "--no-daemon", "-p", directory, "classes", "writeRuntimeClasspath")
      continue
    if builder == "maven":
      directory = build.ROOT / "examples" / project
      # Isolate checkout artifacts from the user's Maven cache.
      build.run("mvn", "-f", directory / "pom.xml",
                f"-Dmaven.repo.local={STAGE / 'm2'}", "-q", "package",
                "org.apache.maven.plugins:maven-dependency-plugin:3.9.0:build-classpath",
                f"-Dmdep.outputFile={directory / 'target/runtime.classpath'}")
      continue
    build.run(build.ELIDE, "-p", build.ROOT / "examples" / project, "install", "--slim", "--direct")
    build.run(build.ELIDE, "-p", build.ROOT / "examples" / project, "build")


def runtime_classpath(project, builder):
  directory = build.ROOT / "examples" / project
  if builder == "gradle":
    return (directory / "build/runtime.classpath").read_text().strip()
  classpath_file = "target/runtime.classpath" if builder == "maven" else ".dev/dependencies/m2/runtime.classpath"
  classes = "target/classes" if builder == "maven" else ".dev/jvm/classes/main/java"
  dependencies = (directory / classpath_file).read_text().strip()
  resources = [] if builder == "maven" else [str(directory / "src/main/resources")]
  return os.pathsep.join([str(directory / classes), *resources, dependencies])


def native_binary(project, builder):
  suffix = ".exe" if os.name == "nt" else ""
  return STAGE / "native" / builder / (project + suffix)


def compile_native(project, builder, optimization="2"):
  cp = runtime_classpath(project, builder)
  # The native executable links the C API and has no dynamic Bemo library resource.
  cp = os.pathsep.join(path for path in cp.split(os.pathsep)
                       if not path.endswith("bemo-native.jar")
                       and not ("bemo-ffm-" in path and build.classifier() in path))
  main = f"dev.elide.bemo.examples.{PROJECTS[project]}.Application"
  if project == "spring-boot":
    aot = STAGE / "aot" / builder / project
    shutil.rmtree(aot, ignore_errors=True)
    source, resources, classes = (aot / name for name in ("sources", "resources", "classes"))
    for directory in (source, resources, classes):
      directory.mkdir(parents=True)
    build.run(build.ELIDE, "java", "--", "-cp", cp,
              "org.springframework.boot.SpringApplicationAotProcessor", main,
              source, resources, classes, "dev.elide.bemo.examples", project)
    # Spring generates Java initializers and reachability metadata. Compile with Elide.
    build.run(build.ELIDE, "javac", "--", "--release", build.VERSIONS["jvm_release"],
              "-proc:none", "-cp", os.pathsep.join([str(classes), cp]),
              "-d", classes, *sorted(source.rglob("*.java")))
    cp = os.pathsep.join([str(classes), str(resources), cp])
  binary = native_binary(project, builder)
  binary.parent.mkdir(parents=True, exist_ok=True)
  linker = {
    "Linux": ["-H:NativeLinkerOption=-ldl", "-H:NativeLinkerOption=-lpthread", "-H:NativeLinkerOption=-lm"],
    "Darwin": [],
    "Windows": ["-H:NativeLinkerOption=ntdll.lib"],
  }[platform.system()]
  build.run(build.java_tool("native-image"), "--no-fallback", f"-O{optimization}",
            f"--parallelism={os.environ.get('BEMO_NATIVE_JOBS', min(8, os.cpu_count() or 1))}",
            "--enable-native-access=ALL-UNNAMED",
            "--initialize-at-run-time=io.netty",
            f"-H:CLibraryPath={STAGE / 'static'}",
            f"--native-compiler-options=-I{STAGE / 'include'}",
            f"-H:ConfigurationFileDirectories={build.ROOT / 'examples/shared/native'}",
            *linker, "-cp", cp, main, binary)


PAYLOAD_PATTERN = b"Hello, World! Bemo framework benchmark.\n"
PAYLOAD = (PAYLOAD_PATTERN * (128 * 1024 // len(PAYLOAD_PATTERN) + 1))[:128 * 1024]


def verify_response(connection, path, enabled, compress=False, secure=False, encoding=None):
  headers = {} if encoding is None else {"Accept-Encoding": encoding}
  connection.request("GET", path, headers=headers)
  response = connection.getresponse()
  wire_body = response.read()
  expected = b"Hello, World!" if path == "/plaintext" else PAYLOAD
  if response.status != 200 or response.will_close:
    raise RuntimeError(f"{path}: status {response.status} or connection closed")
  if not response.getheader("Content-Type", "").startswith("text/plain"):
    raise RuntimeError(f"{path}: incorrect content type")
  if compress:
    if response.getheader("Content-Encoding") != "gzip":
      raise RuntimeError(f"{path}: gzip missing")
    body = gzip.decompress(wire_body)  # Includes member CRC and length validation.
    provider = "bemo-zlib-rs" if enabled else "jdk-zlib"
    if response.getheader("X-Compression-Provider") != provider:
      raise RuntimeError(f"{path}: wrong compression provider")
  else:
    body = wire_body
    if response.getheader("Content-Encoding") is not None:
      raise RuntimeError(f"{path}: unexpected compression")
  if body != expected:
    raise RuntimeError(f"{path}: incorrect decoded body, {len(body)} bytes")
  if "compression" in path and response.getheader("Vary") != "Accept-Encoding":
    raise RuntimeError(f"{path}: missing negotiation metadata")
  if secure:
    provider = "bemo-rustls-aws-lc" if enabled else "jdk"
    if response.getheader("X-TLS-Provider") != provider:
      raise RuntimeError(f"{path}: wrong TLS provider")
  if response.getheader("Content-Length") != str(len(wire_body)):
    raise RuntimeError(f"{path}: incorrect wire content length")


def verify_workloads(port, tls_port, enabled):
  connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
  try:
    verify_response(connection, "/payload", enabled)
    connected = connection.sock
    for encoding, compressed in (("gzip", True), ("gzip;q=0, *;q=1", False), ("*", True), ("br", False), (None, False)):
      verify_response(connection, "/compression", enabled, compressed, encoding=encoding)
      if connection.sock is not connected:
        raise RuntimeError("Compression changed the keep-alive connection")
    for path in ("/tls", "/tls-compression"):
      connection.request("GET", path, headers={"Accept-Encoding": "gzip"})
      response = connection.getresponse()
      response.read()
      if response.status != 426:
        raise RuntimeError(f"{path}: TLS requirement was not enforced")
  finally:
    connection.close()
  certificate = build.ROOT / "examples/shared/src/main/resources/benchmark-tls/localhost-cert.pem"
  for version in (ssl.TLSVersion.TLSv1_2, ssl.TLSVersion.TLSv1_3):
    context = ssl.create_default_context(cafile=certificate)
    context.minimum_version = context.maximum_version = version
    context.set_alpn_protocols(["http/1.1"])
    connection = http.client.HTTPSConnection("127.0.0.1", tls_port, context=context, timeout=5)
    try:
      verify_response(connection, "/tls", enabled, secure=True)
      connected = connection.sock
      expected_version = "TLSv1.2" if version == ssl.TLSVersion.TLSv1_2 else "TLSv1.3"
      if connected.version() != expected_version:
        raise RuntimeError("Wrong negotiated TLS version")
      for _ in range(8):
        verify_response(connection, "/tls-compression", enabled, compress=True, secure=True, encoding="gzip")
        if connection.sock is not connected:
          raise RuntimeError("TLS+compression changed the keep-alive connection")
      verify_response(connection, "/tls-compression", enabled, secure=True, encoding="gzip;q=0")
    finally:
      connection.close()
  # Concurrent requests exercise separate worker-owned compressors and TLS engines.
  def requests(index):
    secure = index % 2 == 0
    connection = (http.client.HTTPSConnection("127.0.0.1", tls_port,
                  context=ssl.create_default_context(cafile=certificate), timeout=5) if secure else
                  http.client.HTTPConnection("127.0.0.1", port, timeout=5))
    try:
      for _ in range(6):
        verify_response(connection, "/tls-compression" if secure else "/compression", enabled,
                        compress=True, secure=secure, encoding="gzip")
    finally:
      connection.close()
  with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
    list(executor.map(requests, range(4)))


def smoke(builder="elide", native=False, projects=None):
  for project in projects or PROJECTS:
    package = PROJECTS[project]
    directory = build.ROOT / "examples" / project
    cp = runtime_classpath(project, builder)
    for enabled in (True, False):
      name = f"{builder}-{'native' if native else 'jvm'}-{project}-{'bemo' if enabled else 'netty'}"
      log = STAGE / f"{name}.log"
      # Port zero lets the OS choose a port, but framework logs differ. Bind a temporary
      # loopback socket to select a port and retry startup requests with a fixed deadline.
      with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
      with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        tls_port = listener.getsockname()[1]
      port_property = "server.port" if project == "spring-boot" else "micronaut.server.port"
      properties = [f"-Dbemo.enabled={str(enabled).lower()}", f"-D{port_property}={port}",
                    "-Dreactor.netty.ioWorkerCount=2", f"-Dbemo.tls.port={tls_port}"]
      command = ([native_binary(project, builder), *properties] if native else
                 [build.java_tool("java"), "--enable-native-access=ALL-UNNAMED", *properties,
                  "-cp", cp, f"dev.elide.bemo.examples.{package}.Application"])
      with log.open("w") as output:
        process = subprocess.Popen(command, cwd=directory, stdout=output, stderr=subprocess.STDOUT)
        try:
          deadline = time.monotonic() + 45
          while True:
            if process.poll() is not None:
              raise RuntimeError(f"{name} exited during startup; see {log.relative_to(build.ROOT)}")
            try:
              connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
              connection.request("GET", "/plaintext")
              response = connection.getresponse()
              body = response.read()
              if response.status != 200 or body != b"Hello, World!":
                raise RuntimeError(f"{name}: unexpected response {response.status}: {body!r}")
              if not response.getheader("Content-Type", "").startswith("text/plain"):
                raise RuntimeError(f"{name}: unexpected content type")
              if response.will_close:
                raise RuntimeError(f"{name}: server disabled keep-alive")
              connected_socket = connection.sock
              # A second request on the same connection exercises keep-alive.
              connection.request("GET", "/plaintext")
              response = connection.getresponse()
              if response.status != 200 or response.read() != b"Hello, World!":
                raise RuntimeError(f"{name}: keep-alive response failed")
              if connection.sock is not connected_socket:
                raise RuntimeError(f"{name}: keep-alive reconnected")
              connection.close()
              break
            except (ConnectionError, TimeoutError, OSError):
              connection.close()
              if time.monotonic() >= deadline:
                raise RuntimeError(f"{name} did not become ready; see {log.relative_to(build.ROOT)}")
              time.sleep(0.1)
          # HTTP readiness can precede the secondary HTTPS listener.
          deadline = time.monotonic() + 30
          while True:
            try:
              with socket.create_connection(("127.0.0.1", tls_port), timeout=1):
                break
            except OSError:
              if process.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError(f"{name}: HTTPS did not start; see {log}")
              time.sleep(0.05)
          verify_workloads(port, tls_port, enabled)
          print(f"{name}: HTTP, gzip, TLS 1.2/1.3, TLS+gzip, concurrency and keep-alive checks passed", flush=True)
        finally:
          process.terminate()
          try:
            process.wait(timeout=20)
          except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise RuntimeError(f"{name}: shutdown timed out")
      transport_enabled = "Bemo transport enabled" in log.read_text()
      if transport_enabled != enabled:
        raise RuntimeError(f"{name}: unexpected transport startup diagnostic")
      if enabled and ("CAPI" if native else "FFM") not in log.read_text():
        raise RuntimeError(f"{name}: unexpected native binding")
      if enabled and not all(marker in log.read_text() for marker in (
          "Bemo compression enabled", "Bemo compression state reclaimed",
          "Bemo TLS enabled", "Bemo TLS workload reclaimed")):
        raise RuntimeError(f"{name}: missing native data-plane/reclamation diagnostic; see {log}")
      if not enabled and any(marker in log.read_text() for marker in ("Bemo compression enabled", "Bemo TLS enabled")):
        raise RuntimeError(f"{name}: native gzip/TLS initialized in stock mode")


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("prepare", "build", "test", "native", "smoke"))
  parser.add_argument("--builder", choices=("elide", "maven", "gradle"), default="elide")
  parser.add_argument("--project", choices=tuple(PROJECTS))
  parser.add_argument("--native", action="store_true", help="Build and test native executables")
  parser.add_argument("--skip-build", action="store_true", help="Use classes prepared by the calling build")
  parser.add_argument("--native-opt", choices=("0", "1", "2", "3", "b"), default="2")
  args = parser.parse_args()
  task = args.task
  if task == "prepare":
    prepare()
  else:
    projects = [args.project] if args.project else list(PROJECTS)
    if not args.skip_build and task != "smoke":
      compile_projects(args.builder, projects)
    if task == "native" or (args.native and task != "smoke"):
      for project in projects:
        compile_native(project, args.builder, args.native_opt)
    if task in ("test", "smoke"):
      smoke(args.builder, args.native, projects)


if __name__ == "__main__":
  main()
