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
  parser.add_argument("--backends", nargs="+", choices=("zlib", "zlib-rs", "zlib-ng"), default=["zlib-rs"])
  parser.add_argument("--levels", nargs="+", type=int, default=[1, 3, 6])
  args = parser.parse_args()
  if not args.levels or any(level < 0 or level > 9 for level in args.levels):
    parser.error("Compression levels must be in 0..9")
  if args.iterations < 1:
    parser.error("Positive iteration count required")
  directory = build.BUILD / "reports/compression"
  directory.mkdir(parents=True, exist_ok=True)
  manifest = build.ROOT / "benchmarks/compression/Cargo.toml"
  rows = []
  for backend in args.backends:
    # Compile first; compilation is neither timed nor overlapped with another backend.
    build.run("cargo", "build", "--release", "--locked", "--manifest-path", manifest,
              "--features", backend)
    executable = manifest.parent / "target/release" / ("bemo-compression-probe.exe" if platform.system() == "Windows" else "bemo-compression-probe")
    text = subprocess.check_output([executable, str(args.iterations), ",".join(map(str, args.levels))], text=True, cwd=build.ROOT)
    (directory / f"{backend}.jsonl").write_text(text)
    rows.extend(json.loads(line) for line in text.splitlines())
  report = {"host": f"{platform.system()}-{platform.machine()}", "iterations": args.iterations,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "scope": "standalone reusable gzip; excludes JVM/FFM/Native Image integration",
            "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
            "levels": args.levels, "cpu": subprocess.check_output(["lscpu"], text=True) if platform.system() == "Linux" else platform.processor(),
            "samples": [row for row in rows if row.get("kind") != "checksum"],
            "checksums": [row for row in rows if row.get("kind") == "checksum"]}
  (directory / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
  rows = report["samples"]
  for backend in args.backends:
    for size in (1024, 65536, 131072):
      for random in (False, True):
        for level in args.levels:
          selected = [row for row in rows if (row["backend"], row["size"], row["random"], row["level"]) == (backend, size, random, level)]
          print(f"{backend:7} level {level} {size:6} {'random' if random else 'json':6}: "
                f"{statistics.median(row['ns_per_member'] for row in selected):.0f} ns, "
                f"{selected[0]['compressed_bytes']} bytes")


if __name__ == "__main__":
  main()
