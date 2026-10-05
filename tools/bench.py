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


def comparisons(summary):
  """Compare like workloads; ratios describe this closed-loop harness only."""
  results = []
  for bemo in summary:
    if bemo["transport"] != "bemo":
      continue
    for other in summary:
      if other["transport"] not in ("epoll", "kqueue", "nio") or other["case"] != bemo["case"]:
        continue
      keys = ("clients", "requests", "warmup_rounds", "tls_provider", "java_version", "host", "workload_sha256")
      if any(bemo["samples"][0][key] != other["samples"][0][key] for key in keys):
        raise RuntimeError("Refusing incompatible transport comparison")
      ratios = [a["requests_per_second"] / z["requests_per_second"]
                for a, z in zip(bemo["samples"], other["samples"], strict=True)]
      results.append({"case": bemo["case"], "comparator": other["transport"],
                      "tls_provider": bemo["tls_provider"], "clients": bemo["samples"][0]["clients"],
                      "bemo_over_comparator_rps": bemo["median_requests_per_second"] / other["median_requests_per_second"],
                      "sample_rps_ratios": ratios,
                      "bemo_over_comparator_p99": bemo["median_latency_p99_ns"] / other["median_latency_p99_ns"]})
  return results


def digest(paths):
  value = hashlib.sha256()
  for path in sorted(paths):
    value.update(str(path.relative_to(build.ROOT)).encode())
    value.update(path.read_bytes())
  return value.hexdigest()


def snapshot():
  sources = [build.ROOT / name for name in ("Cargo.toml", "Cargo.lock", "elide.pkl", "tools/versions.json", "rust-toolchain.toml")]
  sources += sorted((build.ROOT / "tools").glob("*.py"))
  for directory in ("crates", "packages", "benchmarks"):
    # Criterion can fall back to a crate-local target/ when Cargo's environment
    # is unavailable under instrumentation. Generated reports are not inputs.
    for root, directories, files in os.walk(build.ROOT / directory):
      directories[:] = [name for name in directories if name not in ("target", "__pycache__")]
      sources.extend(Path(root) / name for name in files)
  classes = [p for directory in [build.BUILD / "bench/classes", *[build.classes(m) for m in ("api", "ffm", "netty")]]
             for p in directory.rglob("*") if p.is_file()]
  return {"sources_sha256": digest(sources), "classes_sha256": digest(classes),
          "library_sha256": hashlib.sha256(build.library(True).read_bytes()).hexdigest(),
          "source_files": {str(p.relative_to(build.ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                           for p in sorted(sources)}}


def prepare():
  build.rust(release=True)
  build.jvm()
  build.compile_java(build.BUILD / "bench/classes", sorted((build.ROOT / "benchmarks/java").glob("*.java")),
                     [build.classes(name) for name in ("api", "ffm", "netty")] + build.benchmark_netty())
  (build.BUILD / "bench/manifest.json").write_text(json.dumps(snapshot(), indent=2) + "\n")


def measure(case, rounds, warmup, transport="bemo", clients=4, tls_provider="jdk", backend=0):
  tls, gzip, size = CASES[case]
  fixtures = build.ROOT / "crates/bemo/tests/fixtures"
  cp = [build.BUILD / "bench/classes", *[build.classes(name) for name in ("api", "ffm", "netty")], *build.benchmark_netty()]
  java = os.environ.get("BEMO_BENCH_JAVA", os.environ.get("BEMO_TEST_JAVA", str(build.java_tool("java"))))
  command = [java, "-Xms256m", "-Xmx256m", "--enable-native-access=ALL-UNNAMED",
             "-cp", build.classpath(cp), "TransportBenchmark", str(build.library(True)),
             str(fixtures / "localhost-cert.pem"), str(fixtures / "localhost-key.pem"),
             str(tls).lower(), str(gzip).lower(), str(size), str(rounds), str(warmup),
             transport, str(clients), tls_provider, str(backend)]
  result = subprocess.run(command, cwd=build.ROOT, capture_output=True, text=True, timeout=180)
  OUTPUT.mkdir(parents=True, exist_ok=True)
  stamp = time.time_ns()
  name = f"{transport}-{tls_provider}-{clients}-{case}"
  (OUTPUT / f"{name}-{stamp}.log").write_text(result.stdout + result.stderr)
  result.check_returncode()
  metrics = json.loads(next(line for line in result.stdout.splitlines() if line.startswith('{')))
  if metrics["requests"] != rounds * clients or metrics["requests_per_second"] <= 0:
    raise RuntimeError("Incomplete measurement")
  if platform.system() == "Linux" and not metrics["peak_rss_bytes"]:
    raise RuntimeError("Missing RSS measurement")
  metrics.update(workload_sha256=digest([build.ROOT / "benchmarks/java/TransportBenchmark.java", Path(__file__).resolve()]),
                 case=case, requested_backend=backend,
                 commit=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=build.ROOT, text=True).strip(),
                 host=f"{platform.system()}-{platform.machine()}", warmup_rounds=warmup,
                 java_version=subprocess.run([java, "-version"], capture_output=True, text=True, check=True).stderr.strip())
  (OUTPUT / f"{name}-{stamp}.json").write_text(json.dumps(metrics, indent=2) + "\n")
  return metrics


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("prepare", "run"))
  parser.add_argument("--case", choices=tuple(CASES))
  parser.add_argument("--rounds", type=int, default=2500)
  parser.add_argument("--warmup", type=int, default=500)
  parser.add_argument("--samples", type=int, default=3)
  parser.add_argument("--transports", default="bemo,netty-native",
                      help="Comma-separated bemo,netty-native,epoll,kqueue,nio; unavailable backends fail")
  parser.add_argument("--clients", type=int, default=4)
  parser.add_argument("--tls-provider", choices=("jdk", "native"), default="jdk")
  parser.add_argument("--backend", type=int, choices=(0, 1, 2, 3), default=0,
                      help="Bemo backend: 0 AUTO, 1 polling, 2 io_uring, 3 IOCP")
  args = parser.parse_args()
  if args.task == "prepare":
    prepare()
    return
  if min(args.rounds, args.warmup, args.samples, args.clients) < 1 or args.clients > 1024:
    parser.error("Positive workload required; clients must not exceed 1024")
  native = {"Linux": "epoll", "Darwin": "kqueue"}.get(platform.system())
  transports = [native if t == "netty-native" else t for t in args.transports.split(",")]
  if any(t not in ("bemo", "epoll", "kqueue", "nio") for t in transports):
    parser.error("Unknown or unsupported native transport")
  if len(set(transports)) != len(transports):
    parser.error("Duplicate transports")
  if args.tls_provider == "native" and transports != ["bemo"]:
    parser.error("Native TLS requires --transports bemo; use jdk for transport comparisons")
  manifest = build.BUILD / "bench/manifest.json"
  expected = json.loads(manifest.read_text()) if manifest.is_file() else {}
  actual = snapshot()
  changed = [name for name in ("sources_sha256", "classes_sha256", "library_sha256")
             if expected.get(name) != actual[name]]
  if changed:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    (OUTPUT / "manifest-drift.json").write_text(json.dumps(
        {"changed": changed, "source_files_changed": sorted(
            name for name in set(expected.get("source_files", {})) | set(actual["source_files"])
            if expected.get("source_files", {}).get(name) != actual["source_files"].get(name)),
         "expected": expected, "actual": actual}, indent=2) + "\n")
    raise RuntimeError(f"Benchmark inputs or artifacts changed ({', '.join(changed)}); run make bench-prepare")
  report = OUTPUT / (f"summary-{args.case or 'all'}.json")
  report.unlink(missing_ok=True)
  summary = []
  for case in ([args.case] if args.case else CASES):
    measurements = {transport: [] for transport in transports}
    for sample in range(args.samples):
      # Rotate launch order to avoid always measuring the comparator on a warmer host.
      order = transports[sample % len(transports):] + transports[:sample % len(transports)]
      for transport in order:
        measurements[transport].append(measure(case, args.rounds, args.warmup, transport,
                                               args.clients, args.tls_provider, args.backend))
    for transport, samples in measurements.items():
      summary.append({"case": case, "transport": transport, "tls_provider": samples[0]["tls_provider"],
                      "samples": samples,
                      "median_requests_per_second": statistics.median(s["requests_per_second"] for s in samples),
                      "median_latency_p99_ns": statistics.median(s["latency_p99_ns"] for s in samples),
                      "median_cpu_ns_per_request": statistics.median(s["process_cpu_ns"] / s["requests"] for s in samples),
                      "max_peak_rss_bytes": max((s["peak_rss_bytes"] for s in samples if s["peak_rss_bytes"] is not None), default=None)})
  report.write_text(json.dumps(summary, indent=2) + "\n")
  comparison = comparisons(summary)
  report.with_name(report.stem + "-comparison.json").write_text(json.dumps(comparison, indent=2) + "\n")
  print(report.relative_to(build.ROOT))
  if os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as output:
      output.write("| Transport | Workload | TLS provider | Median requests/sec | p99 ns | CPU ns/request | Peak RSS bytes (whole JVM) |\n|---|---|---|---:|---:|---:|---:|\n")
      for case in summary:
        output.write(f"| {case['transport']} | {case['case']} | {case['tls_provider']} | {case['median_requests_per_second']:.1f} | {case['median_latency_p99_ns']} | {case['median_cpu_ns_per_request']:.1f} | {case['max_peak_rss_bytes']} |\n")


if __name__ == "__main__":
  main()
