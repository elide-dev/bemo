#!/usr/bin/env python3
"""Prepare and measure fixed-work JVM/native HTTP workloads; one fresh JVM per sample."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import threading
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
      keys = ("clients", "requests", "warmup_rounds", "java_version", "host", "workload_sha256", "load_generator_transport", "load_generator_tls_provider", "process_scope")
      if any(bemo["samples"][0].get(key) != other["samples"][0].get(key) for key in keys):
        raise RuntimeError("Refusing incompatible transport comparison")
      bemo_tls = bemo["samples"][0]["tls_provider"]
      comparator_tls = other["samples"][0]["tls_provider"]
      if bemo_tls != comparator_tls and (bemo_tls, comparator_tls) != ("native", "jdk"):
        raise RuntimeError("Refusing incompatible transport comparison")
      ratios = [a["requests_per_second"] / z["requests_per_second"]
                for a, z in zip(bemo["samples"], other["samples"], strict=True)]
      results.append({"case": bemo["case"], "comparator": other["transport"],
                      "tls_provider": bemo["tls_provider"], "comparator_tls_provider": comparator_tls,
                      "comparison_kind": "full-stack" if bemo_tls != comparator_tls or any(
                          bemo["samples"][0].get(key) != other["samples"][0].get(key)
                          for key in ("http_provider", "runtime")) else "transport",
                      "bemo_stack": {key: bemo["samples"][0].get(key) for key in ("http_provider", "runtime", "binding")},
                      "comparator_stack": {key: other["samples"][0].get(key) for key in ("http_provider", "runtime", "binding")},
                      "clients": bemo["samples"][0]["clients"],
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
  native = native_server_binary()
  return {"native_server_sha256": hashlib.sha256(native.read_bytes()).hexdigest() if native.is_file() else None,
          "sources_sha256": digest(sources), "classes_sha256": digest(classes),
          "library_sha256": hashlib.sha256(build.library(True).read_bytes()).hexdigest(),
          "source_files": {str(p.relative_to(build.ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                           for p in sorted(sources)}}


def prepare(runtime="native-image"):
  build.rust(release=True)
  build.jvm()
  build.compile_java(build.BUILD / "bench/classes", sorted((build.ROOT / "benchmarks/java").glob("*.java")),
                     [build.classes(name) for name in ("api", "ffm", "netty", "native-image")] + build.sdk() + build.benchmark_netty())
  if runtime == "native-image":
    output = build.BUILD / "bench/native-classes"
    names = ("NativeHttpBenchmarkServer.java", "CapiNativeHttpBenchmarkServer.java")
    cp = [build.classes("api"), build.classes("native-image"), *build.sdk()]
    build.compile_java(output, [build.ROOT / "benchmarks/java" / name for name in names], cp)
    linker = ["-H:NativeLinkerOption=ntdll.lib"] if os.name == "nt" else []
    build.run(build.java_tool("native-image"), "--no-fallback", "-O3",
              "-cp", build.classpath([output, *cp]),
              f"-H:CLibraryPath={build.target_dir(True)}",
              f"--native-compiler-options=-I{build.ROOT / 'include'}", *linker,
              "CapiNativeHttpBenchmarkServer", native_server_binary(), timeout=900)
  (build.BUILD / "bench/manifest.json").write_text(json.dumps(dict(snapshot(), prepared_runtime=runtime), indent=2) + "\n")


def native_server_binary():
  return build.BUILD / "bench" / ("native-http-server.exe" if os.name == "nt" else "native-http-server")


def selected_tls_provider(transport, requested):
  """Bemo uses its Rustls/aws-lc-rs stack; stock Netty uses JDK TLS."""
  if requested == "jdk":
    return "jdk"
  return "native" if transport == "bemo" else "jdk"


def read_json(stream, timeout):
  result = queue.Queue()
  threading.Thread(target=lambda: result.put(stream.readline()), daemon=True).start()
  return json.loads(result.get(timeout=timeout))


def measure(case, rounds, warmup, transport="bemo", clients=4, tls_provider="auto", backend=0,
            http_provider="native", runtime="native-image"):
  tls, gzip, size = CASES[case]
  tls_provider = selected_tls_provider(transport, tls_provider)
  native_http = transport == "bemo" and http_provider == "native"
  if native_http and tls and tls_provider != "native":
    raise ValueError("Native HTTP TLS uses Rustls/aws-lc-rs; select --http-provider netty for JDK TLS")
  fixtures = build.ROOT / "crates/bemo/tests/fixtures"
  cp = [build.BUILD / "bench/classes", *[build.classes(name) for name in ("api", "ffm", "netty")], *build.benchmark_netty()]
  java = os.environ.get("BEMO_BENCH_JAVA", os.environ.get("BEMO_TEST_JAVA", str(build.java_tool("java"))))
  java_args = [java, "-Xms256m", "-Xmx256m", "--enable-native-access=ALL-UNNAMED", "-cp", build.classpath(cp)]
  inputs = [str(build.library(True)), str(fixtures / "localhost-cert.pem"), str(fixtures / "localhost-key.pem"),
            str(tls).lower(), str(gzip).lower(), str(size)]
  client_transport = {"Linux": "epoll", "Darwin": "kqueue"}.get(platform.system())
  if client_transport is None:
    raise RuntimeError("Native Netty load generator requires Linux or macOS")
  if native_http:
    server_command = ([str(native_server_binary()), "-Xms256m", "-Xmx256m"] if runtime == "native-image"
                      else [*java_args, "FfmNativeHttpBenchmarkServer"])
    server_command += [*inputs, str(backend)]
  else:
    server_command = [*java_args, "TransportBenchmark", *inputs, str(rounds), str(warmup),
                      transport, str(clients), tls_provider, str(backend), "server"]
  OUTPUT.mkdir(parents=True, exist_ok=True)
  stamp = time.time_ns()
  actual_runtime = runtime if native_http else "jvm"
  name = f"{transport}-{actual_runtime}-{'native' if native_http else 'netty'}-{tls_provider}-{clients}-{case}"
  server_log = OUTPUT / f"{name}-{stamp}-server.log"
  with server_log.open("w") as log:
    server = subprocess.Popen(server_command, cwd=build.ROOT, stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=log, text=True)
    try:
      ready = read_json(server.stdout, 60)
      log.write(json.dumps(ready) + "\n")
      log.flush()
      command = [*java_args, "TransportBenchmark", *inputs, str(rounds), str(warmup),
                 client_transport, str(clients), "jdk", "0", str(ready["port"]), str(server.pid)]
      client_log = OUTPUT / f"{name}-{stamp}.log"
      with client_log.open("w") as output:
        client = subprocess.Popen(command, cwd=build.ROOT, stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=output, text=True)
        try:
          start = read_json(client.stdout, 180)
          if start != {"phase": "start"}:
            raise RuntimeError("Client did not finish warmup")
          server.stdin.write("cpu\n")
          server.stdin.flush()
          cpu_before = read_json(server.stdout, 10)["server_cpu_ns"]
          client.stdin.write("\n")
          client.stdin.flush()
          metrics = read_json(client.stdout, 180)
          server.stdin.write("cpu\n")
          server.stdin.flush()
          cpu_after = read_json(server.stdout, 10)["server_cpu_ns"]
          metrics["server_cpu_ns"] = cpu_after - cpu_before
          metrics["client_cpu_ns"] = metrics["process_cpu_ns"]
          metrics["process_cpu_ns"] += metrics["server_cpu_ns"]
          output.write(json.dumps(metrics) + "\n")
          client.stdin.write("\n")
          client.stdin.flush()
          client.stdin.close()
          if client.wait(timeout=15) != 0:
            raise RuntimeError(f"Client failed; inspect {client_log}")
        finally:
          if client.poll() is None:
            client.terminate()
            try:
              client.wait(timeout=5)
            except subprocess.TimeoutExpired:
              client.kill()
              client.wait()
          client.stdout.close()
          if not client.stdin.closed:
            client.stdin.close()
    finally:
      server.stdin.close()
      try:
        server.wait(timeout=15)
      except subprocess.TimeoutExpired:
        server.terminate()
        try:
          server.wait(timeout=5)
        except subprocess.TimeoutExpired:
          server.kill()
          server.wait()
      log.write(server.stdout.read())
      server.stdout.close()
    if server.returncode != 0:
      raise RuntimeError(f"Server failed or did not reclaim native storage; inspect {server_log}")
  if metrics["requests"] != rounds * clients or metrics["requests_per_second"] <= 0:
    raise RuntimeError("Incomplete measurement")
  if platform.system() == "Linux" and not metrics["peak_rss_bytes"]:
    raise RuntimeError("Missing RSS measurement")
  metrics.update(transport=transport, driver=ready["driver"], auto_fallback=ready["auto_fallback"],
                 tls_provider=tls_provider if tls else "none", http_provider="native" if native_http else "netty",
                 runtime=actual_runtime, binding="capi" if native_http and runtime == "native-image" else "ffm" if transport == "bemo" else "netty",
                 load_generator_transport=client_transport, load_generator_tls_provider="jdk" if tls else "none",
                 process_scope="server+client", gzip_provider="java.util.zip" if native_http and gzip else "netty" if gzip else "none",
                 workload_sha256=digest([*sorted((build.ROOT / "benchmarks/java").glob("*.java")), Path(__file__).resolve()]),
                 case=case, requested_backend=backend if transport == "bemo" else 0,
                 commit=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=build.ROOT, text=True).strip(),
                 host=f"{platform.system()}-{platform.machine()}", warmup_rounds=warmup,
                 java_version=subprocess.run([java, "-version"], capture_output=True, text=True, check=True).stderr.strip())
  (OUTPUT / f"{name}-{stamp}.json").write_text(json.dumps(metrics, indent=2) + "\n")
  return metrics


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("task", choices=("prepare", "run"))
  parser.add_argument("--case", choices=tuple(CASES))
  parser.add_argument("--rounds", type=int, default=25000)
  parser.add_argument("--warmup", type=int, default=5000)
  parser.add_argument("--samples", type=int, default=3)
  parser.add_argument("--transports", default="bemo,netty-native",
                      help="Comma-separated bemo,netty-native,epoll,kqueue,nio; unavailable backends fail")
  parser.add_argument("--clients", type=int, default=4)
  parser.add_argument("--tls-provider", choices=("auto", "jdk", "native"), default="auto",
                      help="Default: Bemo Rustls/aws-lc-rs vs Netty JDK; jdk isolates transport costs")
  parser.add_argument("--http-provider", choices=("native", "netty"), default="native",
                      help="Bemo V2 native HTTP by default; netty is an explicit transport control")
  parser.add_argument("--runtime", choices=("native-image", "jvm"), default="native-image",
                      help="Bemo native HTTP server runtime; comparator and load generator use stock OpenJDK")
  parser.add_argument("--backend", type=int, choices=(0, 1, 2, 3), default=0,
                      help="Bemo backend: 0 AUTO, 1 polling, 2 io_uring, 3 IOCP")
  args = parser.parse_args()
  if args.task == "prepare":
    prepare(args.runtime)
    return
  if min(args.rounds, args.warmup, args.samples, args.clients) < 1 or args.clients > 1024:
    parser.error("Positive workload required; clients must not exceed 1024")
  native = {"Linux": "epoll", "Darwin": "kqueue"}.get(platform.system())
  transports = [native if t == "netty-native" else t for t in args.transports.split(",")]
  if any(t not in ("bemo", "epoll", "kqueue", "nio") for t in transports):
    parser.error("Unknown or unsupported native transport")
  if len(set(transports)) != len(transports):
    parser.error("Duplicate transports")
  if args.http_provider == "native" and args.tls_provider == "jdk" and "bemo" in transports:
    parser.error("Use --http-provider netty --runtime jvm for JDK TLS transport controls")
  manifest = build.BUILD / "bench/manifest.json"
  expected = json.loads(manifest.read_text()) if manifest.is_file() else {}
  if args.http_provider == "native" and args.runtime == "native-image" and "bemo" in transports:
    if expected.get("prepared_runtime") != "native-image":
      raise RuntimeError("Run make bench-prepare to build the optimized Native Image HTTP server")
  actual = snapshot()
  changed = [name for name in ("sources_sha256", "classes_sha256", "library_sha256", "native_server_sha256")
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
                                               args.clients, args.tls_provider, args.backend, args.http_provider, args.runtime))
    for transport, samples in measurements.items():
      summary.append({"case": case, "transport": transport, "tls_provider": samples[0]["tls_provider"],
                      "http_provider": samples[0]["http_provider"], "runtime": samples[0]["runtime"],
                      "binding": samples[0]["binding"],
                      "samples": samples,
                      "median_requests_per_second": statistics.median(s["requests_per_second"] for s in samples),
                      "median_latency_p99_ns": statistics.median(s["latency_p99_ns"] for s in samples),
                      "median_cpu_ns_per_request": statistics.median(s["process_cpu_ns"] / s["requests"] for s in samples),
                      "median_server_cpu_ns_per_request": statistics.median(s["server_cpu_ns"] / s["requests"] for s in samples),
                      "median_client_cpu_ns_per_request": statistics.median(s["client_cpu_ns"] / s["requests"] for s in samples),
                      "max_peak_rss_bytes": max((s["peak_rss_bytes"] for s in samples if s["peak_rss_bytes"] is not None), default=None)})
  report.write_text(json.dumps(summary, indent=2) + "\n")
  comparison = comparisons(summary)
  report.with_name(report.stem + "-comparison.json").write_text(json.dumps(comparison, indent=2) + "\n")
  print(report.relative_to(build.ROOT))
  if os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as output:
      output.write("| Transport | Workload | HTTP | Runtime | TLS | Median requests/sec | p99 ns | CPU ns/request (server+client) | Sum RSS high-water marks |\n|---|---|---|---|---|---:|---:|---:|---:|\n")
      for case in summary:
        output.write(f"| {case['transport']} | {case['case']} | {case['http_provider']} | {case['runtime']} | {case['tls_provider']} | {case['median_requests_per_second']:.1f} | {case['median_latency_p99_ns']} | {case['median_cpu_ns_per_request']:.1f} | {case['max_peak_rss_bytes']} |\n")


if __name__ == "__main__":
  main()
