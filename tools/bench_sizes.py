#!/usr/bin/env python3
"""Verify actual native HTTP and Netty gzip bodies at levels 1 and 6."""
import gzip
import hashlib
import http.client
import json
import os
from pathlib import Path
import subprocess

import bench
import build


def main():
  output = build.BUILD / "reports/compression-sizes"
  output.mkdir(parents=True, exist_ok=True)
  fixtures = build.ROOT / "crates/bemo/tests/fixtures"
  cp = [build.BUILD / "bench/classes", *[build.classes(name) for name in ("api", "ffm", "netty")], *build.benchmark_netty()]
  java = os.environ.get("BEMO_BENCH_JAVA", str(build.java_tool("java")))
  results = []
  for size in (1024, 65536, 131072):
    pattern = b'{"message":"bemo transport benchmark","value":12345}\n'
    expected = (pattern * (size // len(pattern) + 1))[:size]
    for transport, level in (("bemo", 1), ("epoll", 1), ("epoll", 6)):
      inputs = [str(build.library(True)), str(fixtures / "localhost-cert.pem"), str(fixtures / "localhost-key.pem"), "false", "true", str(size)]
      command = ([str(bench.native_server_binary()), *inputs, "2"] if transport == "bemo" else
                 [java, "--enable-native-access=ALL-UNNAMED", "-cp", build.classpath(cp), "TransportBenchmark", *inputs, "1", "1", transport, "1", "jdk", "0", "server"])
      with (output / f"{transport}-{level}-{size}.log").open("w") as log:
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True,
                                   env=dict(os.environ, BEMO_BENCH_GZIP_LEVEL=str(level)))
        try:
          ready = bench.read_json(process.stdout, 60)
          if ready["gzip_level"] != level or ready["auto_fallback"]:
            raise RuntimeError(f"Unexpected compression configuration: {ready}")
          connection = http.client.HTTPConnection("127.0.0.1", ready["port"], timeout=10)
          sizes = []
          for _ in range(3):
            connection.request("GET", "/payload", headers={"Accept-Encoding": "gzip"})
            response = connection.getresponse()
            encoded = response.read()
            if response.status != 200 or response.getheader("Content-Encoding") != "gzip" or gzip.decompress(encoded) != expected:
              raise RuntimeError("Invalid compressed response")
            sizes.append(len(encoded))
          connection.close()
          if len(set(sizes)) != 1:
            raise RuntimeError("Non-deterministic compressed size")
          results.append(dict(transport=transport, gzip_level=level, payload_bytes=size,
                              wire_body_bytes=sizes[0], payload_sha256=hashlib.sha256(expected).hexdigest(),
                              validated_responses=len(sizes), driver=ready["driver"]))
        finally:
          process.stdin.close()
          process.wait(timeout=20)
          process.stdout.close()
        if process.returncode:
          raise RuntimeError(f"Server failed: {command}")
  (output / "sizes.json").write_text(json.dumps(results, indent=2) + "\n")
  print(json.dumps(results, indent=2))


if __name__ == "__main__":
  main()
