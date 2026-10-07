#!/usr/bin/env python3
"""Compare reusable gzip backends without changing the JVM's compression provider."""
import argparse
import json
import platform
import statistics
import subprocess

import build


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("--iterations", type=int, default=10000)
  args = parser.parse_args()
  if args.iterations < 1:
    parser.error("Positive iteration count required")
  directory = build.BUILD / "reports/compression"
  directory.mkdir(parents=True, exist_ok=True)
  manifest = build.ROOT / "benchmarks/compression/Cargo.toml"
  rows = []
  for backend in ("zlib", "zlib-rs", "zlib-ng"):
    # Compile first; compilation is neither timed nor overlapped with another backend.
    build.run("cargo", "build", "--release", "--locked", "--manifest-path", manifest,
              "--features", backend)
    executable = manifest.parent / "target/release" / ("bemo-compression-probe.exe" if platform.system() == "Windows" else "bemo-compression-probe")
    text = subprocess.check_output([executable, str(args.iterations)], text=True, cwd=build.ROOT)
    (directory / f"{backend}.jsonl").write_text(text)
    rows.extend(json.loads(line) for line in text.splitlines())
  report = {"host": f"{platform.system()}-{platform.machine()}", "iterations": args.iterations,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "scope": "standalone reusable gzip; excludes JVM/FFM/Native Image integration", "samples": rows}
  (directory / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
  for backend in ("zlib", "zlib-rs", "zlib-ng"):
    for size in (1024, 65536):
      for random in (False, True):
        selected = [row for row in rows if (row["backend"], row["size"], row["random"]) == (backend, size, random)]
        print(f"{backend:7} {size:5} {'random' if random else 'json':6}: "
              f"{statistics.median(row['ns_per_member'] for row in selected):.0f} ns, "
              f"{selected[0]['compressed_bytes']} bytes")


if __name__ == "__main__":
  main()
