#!/usr/bin/env python3
"""Prepare and measure fixed-work JVM/native HTTP workloads; one fresh JVM per sample."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time

import build

CASES = {f"{'tls' if tls else 'plain'}-{'gzip' if gzip else 'identity'}-{size}": (tls, gzip, size)
         for tls in (False, True) for gzip in (False, True) for size in (1024, 65536)}
OUTPUT = build.BUILD / "reports/benchmarks"


def digest(paths):
  value = hashlib.sha256()
  for path in sorted(paths):
    value.update(str(path.relative_to(build.ROOT)).encode())
    value.update(path.read_bytes())
  return value.hexdigest()


def snapshot():
  sources = [build.ROOT / name for name in ("Cargo.toml", "Cargo.lock", "elide.pkl", "tools/versions.json", "rust-toolchain.toml")]
  for directory in ("crates", "packages", "benchmarks"):
    # Criterion can fall back to a crate-local target/ when Cargo's environment
    # is unavailable under instrumentation. Generated reports are not inputs.
    for root, directories, files in os.walk(build.ROOT / directory):
      directories[:] = [name for name in directories if name not in ("target", "__pycache__")]
      sources.extend(Path(root) / name for name in files)
  classes = [p for directory in [build.BUILD / "bench/classes", *[build.classes(m) for m in ("api", "ffm", "netty")]]
             for p in directory.rglob("*") if p.is_file()]
  return {"sources_sha256": digest(sources), "classes_sha256": digest(classes),
          "library_sha256": hashlib.sha256(build.library(True).read_bytes()).hexdigest()}


def prepare():
  build.rust(release=True)
  build.jvm()
  build.compile_java(build.BUILD / "bench/classes", sorted((build.ROOT / "benchmarks/java").glob("*.java")),
                     [build.classes(name) for name in ("api", "ffm", "netty")] + build.netty())
  (build.BUILD / "bench/manifest.json").write_text(json.dumps(snapshot(), indent=2) + "\n")


def measure(case, rounds, warmup):
  tls, gzip, size = CASES[case]
  fixtures = build.ROOT / "crates/dokar/tests/fixtures"
  cp = [build.BUILD / "bench/classes", *[build.classes(name) for name in ("api", "ffm", "netty")], *build.netty()]
  command = [str(build.java_tool("java")), "-Xms256m", "-Xmx256m", "--enable-native-access=ALL-UNNAMED",
             "-cp", build.classpath(cp), "TransportBenchmark", str(build.library(True)),
             str(fixtures / "localhost-cert.pem"), str(fixtures / "localhost-key.pem"),
             str(tls).lower(), str(gzip).lower(), str(size), str(rounds), str(warmup)]
  result = subprocess.run(command, cwd=build.ROOT, capture_output=True, text=True, timeout=180)
  OUTPUT.mkdir(parents=True, exist_ok=True)
  stamp = time.time_ns()
  (OUTPUT / f"{case}-{stamp}.log").write_text(result.stdout + result.stderr)
  result.check_returncode()
  metrics = json.loads(next(line for line in result.stdout.splitlines() if line.startswith('{')))
  if metrics["requests"] != rounds * 4 or metrics["requests_per_second"] <= 0:
    raise RuntimeError("Incomplete measurement")
  if platform.system() == "Linux" and not metrics["peak_rss_bytes"]:
    raise RuntimeError("Missing RSS measurement")
  metrics.update(workload_sha256=digest([build.ROOT / "benchmarks/java/TransportBenchmark.java", Path(__file__).resolve()]),
                 case=case, commit=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=build.ROOT, text=True).strip(),
                 host=f"{platform.system()}-{platform.machine()}", warmup_rounds=warmup,
                 java_version=subprocess.run([str(build.java_tool("java")), "-version"], capture_output=True, text=True, check=True).stderr.splitlines()[0])
  (OUTPUT / f"{case}-{stamp}.json").write_text(json.dumps(metrics, indent=2) + "\n")
  return metrics


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("prepare", "run"))
  parser.add_argument("--case", choices=tuple(CASES))
  parser.add_argument("--rounds", type=int, default=2500)
  parser.add_argument("--warmup", type=int, default=500)
  parser.add_argument("--samples", type=int, default=3)
  args = parser.parse_args()
  if args.task == "prepare":
    prepare()
    return
  if min(args.rounds, args.warmup, args.samples) < 1:
    parser.error("rounds, warmup and samples must be positive")
  manifest = build.BUILD / "bench/manifest.json"
  expected = json.loads(manifest.read_text()) if manifest.is_file() else {}
  actual = snapshot()
  changed = [name for name in actual if expected.get(name) != actual[name]]
  if changed:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    (OUTPUT / "manifest-drift.json").write_text(json.dumps(
        {"changed": changed, "expected": expected, "actual": actual}, indent=2) + "\n")
    raise RuntimeError(f"Benchmark inputs or artifacts changed ({', '.join(changed)}); run make bench-prepare")
  report = OUTPUT / (f"summary-{args.case or 'all'}.json")
  report.unlink(missing_ok=True)
  summary = []
  for case in ([args.case] if args.case else CASES):
    samples = [measure(case, args.rounds, args.warmup) for _ in range(args.samples)]
    summary.append({"case": case, "samples": samples,
                    "median_requests_per_second": statistics.median(s["requests_per_second"] for s in samples),
                    "max_peak_rss_bytes": max((s["peak_rss_bytes"] for s in samples if s["peak_rss_bytes"] is not None), default=None)})
  report.write_text(json.dumps(summary, indent=2) + "\n")
  print(report.relative_to(build.ROOT))
  if os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as output:
      output.write("| Workload | Median requests/sec | Peak RSS bytes (whole JVM) |\n|---|---:|---:|\n")
      for case in summary:
        output.write(f"| {case['case']} | {case['median_requests_per_second']:.1f} | {case['max_peak_rss_bytes']} |\n")


if __name__ == "__main__":
  main()
