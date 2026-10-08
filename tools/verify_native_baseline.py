#!/usr/bin/env python3
"""Check native Netty HTTP/TLS and missing-library rejection without measuring performance."""
import gzip
import http.client
import json
import os
import sys
import platform
import ssl
import subprocess
import tempfile

import build
import bench


def verify():
  backend = {"Darwin": "kqueue", "Linux": "epoll"}.get(platform.system())
  if backend is None:
    raise RuntimeError("Native baseline qualification requires Linux or macOS")
  dependencies = [*[build.classes(name) for name in build.MODULES], *build.sdk(), *build.benchmark_netty()]
  output = build.BUILD / "baseline-check/classes"
  build.compile_java(output, sorted((build.ROOT / "benchmarks/java").glob("*.java")), dependencies)
  cp = [output, *dependencies]
  fixtures = build.ROOT / "examples/shared/src/main/resources/benchmark-tls"
  certificate = fixtures / "localhost-cert.pem"
  inputs = [str(build.library(True)), str(certificate), str(fixtures / "localhost-key.pem")]

  def command(classpath, tls):
    # Server-only mode never calls warmup/exercise or gathers timing samples.
    return [str(build.java_tool("java")), "--enable-native-access=ALL-UNNAMED", "-cp",
            build.classpath(classpath), "TransportBenchmark", *inputs,
            str(tls).lower(), "true", "1024", "1", "1", backend, "1", "openssl", "0", "server"]

  pattern = b'{"message":"bemo transport benchmark","value":12345}\n'
  expected = (pattern * (1024 // len(pattern) + 1))[:1024]
  for tls in (False, True):
    with tempfile.TemporaryFile(mode="w+") as errors:
      process = subprocess.Popen(command(cp, tls), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=errors, text=True)
      try:
        ready = bench.read_json(process.stdout, 30)
        channel = f"io.netty.channel.{backend}.{'Epoll' if backend == 'epoll' else 'KQueue'}ServerSocketChannel"
        if (ready["driver"], ready["server_channel"], ready["tls_provider"], ready["auto_fallback"]) != (
            backend, channel, "openssl" if tls else "none", False):
          raise RuntimeError("Unexpected native baseline: " + json.dumps(ready))
        if tls:
          context = ssl.create_default_context(cafile=certificate)
          context.minimum_version = context.maximum_version = ssl.TLSVersion.TLSv1_3
          connection = http.client.HTTPSConnection("127.0.0.1", ready["port"], context=context, timeout=5)
        else:
          connection = http.client.HTTPConnection("127.0.0.1", ready["port"], timeout=5)
        try:
          for _ in range(2):
            connection.request("GET", "/", headers={"Accept-Encoding": "gzip"})
            response = connection.getresponse()
            if response.status != 200 or gzip.decompress(response.read()) != expected:
              raise RuntimeError("Incorrect native baseline response")
            if tls and (connection.sock.version(), connection.sock.cipher()[0]) != (
                "TLSv1.3", "TLS_AES_128_GCM_SHA256"):
              raise RuntimeError("Incorrect native baseline TLS policy")
        finally:
          connection.close()
        process.stdin.close()
        if process.wait(timeout=20) != 0:
          raise RuntimeError("Native baseline failed shutdown")
        print("Native baseline correctness: " + json.dumps(ready), flush=True)
      except Exception:
        errors.seek(0)
        print(errors.read(), flush=True)
        raise
      finally:
        if process.poll() is None:
          process.kill()
          process.wait()
        process.stdout.close()
        if not process.stdin.closed:
          process.stdin.close()
  for artifact, tls in (("netty-transport-native-" + backend, False), ("netty-tcnative-boringssl-static", True)):
    missing = [path for path in cp if artifact not in path.name]
    result = subprocess.run(command(missing, tls), input="", text=True, capture_output=True, timeout=20)
    if result.returncode == 0 or result.stdout or "UnsatisfiedLinkError" not in result.stderr:
      raise RuntimeError("Missing native library did not prevent startup: " + artifact)
    print("Missing native dependency rejected: " + artifact, flush=True)


if __name__ == "__main__":
  policy = str(build.ROOT / "benchmarks/openssl.cnf")
  if os.environ.get("OPENSSL_CONF") != policy:
    # OpenSSL initializes before Python exposes SSLContext; TLS 1.3 ciphers need its config.
    os.execve(sys.executable, [sys.executable, *sys.argv], dict(os.environ, OPENSSL_CONF=policy))
  verify()
