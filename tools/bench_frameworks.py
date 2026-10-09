#!/usr/bin/env python3
"""Measure prepared framework examples on Linux using a validating wrk client."""
import argparse
import hashlib
import http.client
import itertools
import json
import os
from pathlib import Path
import platform
import signal
import socket
import ssl
import statistics
import sys
import subprocess
import time

import build
import examples

TLS_CIPHER = "TLS_AES_128_GCM_SHA256"
TLS_CONFIG = build.ROOT / "benchmarks/openssl.cnf"
TLS_SHIM = build.BUILD / "bench/openssl-policy.so"

WORKLOADS = {
  "plaintext": ("/plaintext", False, False, 13),
  "payload": ("/payload", False, False, 128 * 1024),
  "compression": ("/compression", False, True, 128 * 1024),
  "tls": ("/tls", True, False, 128 * 1024),
  "tls-compression": ("/tls-compression", True, True, 128 * 1024),
}


def capture(*command):
  result = subprocess.run(command, capture_output=True, text=True, check=True)
  return (result.stdout + result.stderr).strip()


def fingerprint(paths):
  return {str(path): hashlib.sha256(path.read_bytes()).hexdigest()
          for path in sorted(set(paths)) if path.is_file()}


def process_stats(pid):
  status = Path(f"/proc/{pid}/status").read_text()
  memory = {line.split(':')[0]: int(line.split()[1]) * 1024
            for line in status.splitlines() if line.startswith(("VmRSS:", "VmHWM:"))}
  # Fields after the closing parenthesis begin at stat field 3.
  fields = Path(f"/proc/{pid}/stat").read_text().rsplit(')', 1)[1].split()
  return {"rss_bytes": memory["VmRSS"], "peak_rss_bytes": memory["VmHWM"],
          "cpu_seconds": (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")}


def ready(process, port, args, enabled):
  path, tls, compress, _ = WORKLOADS[args.workload]
  context = ssl.create_default_context(cafile=build.ROOT / "examples/shared/src/main/resources/benchmark-tls/localhost-cert.pem") if tls else None
  deadline = time.monotonic() + 60
  while process.poll() is None:
    connection = (http.client.HTTPSConnection("127.0.0.1", port, context=context, timeout=2) if tls else
                  http.client.HTTPConnection("127.0.0.1", port, timeout=2))
    try:
      examples.verify_response(connection, path, enabled, compress, tls, "gzip" if compress else None)
      original = connection.sock
      examples.verify_response(connection, path, enabled, compress, tls, "gzip" if compress else None)
      if connection.sock is not original:
        raise RuntimeError("Incorrect persistent-connection reply")
      security = {"tls_protocol": connection.sock.version(), "tls_cipher": connection.sock.cipher()[0]} if tls else {}
      if tls and security != {"tls_protocol": "TLSv1.3", "tls_cipher": TLS_CIPHER}:
        raise RuntimeError(f"Unexpected benchmark TLS policy: {security}")
      return security
    except (OSError, TimeoutError):
      if time.monotonic() >= deadline:
        raise RuntimeError("Server startup timed out")
      time.sleep(0.05)
    finally:
      connection.close()
  raise RuntimeError("Server exited before readiness")


def load(args, port, duration, log, transport="bemo"):
  path, tls, compress, size = WORKLOADS[args.workload]
  compression_provider = "bemo-zlib-rs" if transport == "bemo" else "jdk-zlib"
  tls_provider = ("bemo-rustls-aws-lc" if transport == "bemo" else "netty-tcnative") if tls else "none"
  command = ["taskset", "-c", args.client_cpus, "wrk", "-t", str(args.threads),
             "-c", str(args.connections), "-d", f"{duration}s", "--timeout", "2s",
             "--latency", "-s", str(build.ROOT / "benchmarks/framework.lua"),
             *( ["-H", "Accept-Encoding: gzip"] if compress else []),
             f"{'https' if tls else 'http'}://127.0.0.1:{port}{path}",
             "--", str(size), "gzip" if compress else "identity", compression_provider, tls_provider]
  with log.open("w") as output:
    environment = dict(os.environ)
    if tls:
      if not TLS_SHIM.is_file():
        raise RuntimeError("Build the benchmark-only OpenSSL policy shim before measuring TLS")
      environment["LD_PRELOAD"] = str(TLS_SHIM)
    subprocess.run(command, stdout=output, stderr=subprocess.STDOUT, check=True, timeout=duration + 30, env=environment)
  lines = [line.removeprefix("BEMO_RESULT ") for line in log.read_text().splitlines()
           if line.startswith("BEMO_RESULT ")]
  if len(lines) != 1:
    raise RuntimeError(f"Missing wrk result: {log}")
  result = json.loads(lines[0])
  if result["requests"] <= 0 or any(result[key] for key in (
      "invalid_responses", "connect_errors", "read_errors", "write_errors", "status_errors", "timeouts")):
    raise RuntimeError(f"Invalid load sample: {log}: {result}")
  return result


def measure(args, case, repetition, output):
  project, runtime, transport = case
  name = f"{project}-{runtime}-{transport}-{repetition}"
  with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
  with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    tls_port = listener.getsockname()[1]
  path, tls, compress, size = WORKLOADS[args.workload]
  selected_port = tls_port if tls else port
  prop = "server.port" if project in ("spring-boot", "ktor") else "micronaut.server.port"
  properties = [f"-Dbemo.enabled={str(transport == 'bemo').lower()}", f"-D{prop}={port}",
                "-Dreactor.netty.ioWorkerCount=2", f"-Dbemo.tls.port={tls_port}",
                f"-Dbemo.tls.enabled={str(tls).lower()}", f"-Dbemo.gzip.level={args.gzip_level}"]
  heap = ["-Xms256m", "-Xmx256m"]
  if runtime == "native":
    executable = examples.native_binary(project, args.builder)
    command = [str(executable), *heap, *properties]
  else:
    command = [str(build.java_tool("java")), *heap, "--enable-native-access=ALL-UNNAMED",
               *properties, "-cp", examples.runtime_classpath(project, args.builder),
               f"dev.elide.bemo.examples.{examples.PROJECTS[project]}.Application"]
  command = ["taskset", "-c", args.server_cpus, *command]
  if args.probe:
    command = ["strace", "-f", "-e",
               "trace=io_uring_setup,io_uring_enter,epoll_create1,epoll_wait,epoll_pwait,epoll_pwait2",
               "-o", str(output / f"{name}.strace"), *command]
  server_log = output / f"{name}-server.log"
  started = time.monotonic()
  with server_log.open("w") as log:
    process = subprocess.Popen(command, cwd=build.ROOT / "examples" / project,
                               stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    try:
      security = ready(process, selected_port, args, transport == "bemo")
      evidence = {} if transport == "bemo" else examples.baseline_evidence(server_log.read_text(), secure=tls)
      security.update(evidence)
      startup_ms = (time.monotonic() - started) * 1000
      if args.probe:
        metrics = {"probe_only": True}
      else:
        load(args, selected_port, args.warmup, output / f"{name}-warmup.log", transport)
        before = process_stats(process.pid)
        metrics = load(args, selected_port, args.duration, output / f"{name}-wrk.log", transport)
        after = process_stats(process.pid)
        metrics.update(server_before=before, server_after=after,
                       server_cpu_ns_per_request=(after["cpu_seconds"] - before["cpu_seconds"]) * 1e9 / metrics["requests"])
      metrics.update(security)
      metrics.update(framework=project, runtime=runtime, transport=transport,
                     workload=args.workload, payload_bytes=size, tls=tls, compression=compress,
                     gzip_level=args.gzip_level if compress else None,
                     compression_provider=("bemo-zlib-rs" if transport == "bemo" else "jdk-zlib") if compress else "none",
                     tls_provider=("bemo-rustls-aws-lc" if transport == "bemo" else "netty-tcnative") if tls else "none",
                     repetition=repetition, startup_to_verified_http_ms=startup_ms,
                     command=command, server_pid=process.pid)
    finally:
      if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
      try:
        process.wait(timeout=25)
      except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        raise RuntimeError(f"Server shutdown timed out: {server_log}")
  text = server_log.read_text()
  if ("Bemo transport enabled" in text) != (transport == "bemo"):
    raise RuntimeError(f"Unexpected transport diagnostic: {server_log}")
  if transport == "bemo" and ("CAPI" if runtime == "native" else "FFM") not in text:
    raise RuntimeError(f"Unexpected binding: {server_log}")
  if transport == "bemo" and ((compress and "Bemo compression enabled" not in text)
                              or (tls and "Bemo TLS enabled" not in text)):
    raise RuntimeError(f"Native gzip/TLS data plane missing: {server_log}")
  metrics["driver_fallback_log"] = [line for line in text.splitlines() if "io_uring is unavailable" in line]
  (output / f"{name}.json").write_text(json.dumps(metrics, indent=2) + "\n")
  print(name, metrics.get("requests_per_second", "probe passed"), flush=True)
  return metrics


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("--builder", choices=("elide", "maven", "gradle"), default="elide")
  parser.add_argument("--workload", choices=tuple(WORKLOADS), default="plaintext")
  parser.add_argument("--gzip-level", type=int, choices=range(10), default=1,
                      help="Matched native and JDK gzip level (default 1)")
  parser.add_argument("--warmup", type=int, default=20)
  parser.add_argument("--duration", type=int, default=20)
  parser.add_argument("--samples", type=int, default=3)
  parser.add_argument("--connections", type=int, default=64)
  parser.add_argument("--threads", type=int, default=4)
  parser.add_argument("--server-cpus", required=True)
  parser.add_argument("--client-cpus", required=True)
  parser.add_argument("--probe", action="store_true", help="Trace backend syscalls separately; never measure under strace")
  parser.add_argument("--output", type=Path, required=True)
  args = parser.parse_args()
  if WORKLOADS[args.workload][1] and os.environ.get("OPENSSL_CONF") != str(TLS_CONFIG):
    # OpenSSL reads its system policy at initialization, before importing ssl.
    os.execve(sys.executable, [sys.executable, *sys.argv], dict(os.environ, OPENSSL_CONF=str(TLS_CONFIG)))
  if platform.system() != "Linux":
    parser.error("This runner requires Linux /proc and taskset")
  if min(args.warmup, args.duration, args.samples, args.connections, args.threads) < 1:
    parser.error("Counts and durations must be positive")
  output = args.output.resolve()
  output.mkdir(parents=True, exist_ok=False)
  cases = list(itertools.product(examples.PROJECTS, ("jvm", "native"), ("bemo", "netty")))
  artifacts = []
  for project in examples.PROJECTS:
    artifacts += [Path(p) for p in examples.runtime_classpath(project, args.builder).split(os.pathsep)]
    artifacts += list(examples.native_binary(project, args.builder).parent.glob("*"))
  # Hash compiled classes and resources for every builder, not only dependency JARs.
  directories = [p for p in artifacts if p.is_dir()]
  artifacts += [file for directory in directories for file in directory.rglob("*") if file.is_file()]
  artifacts += [build.library(True)]
  metadata = {"arguments": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
              "tls_client_policy": {"protocol": "TLSv1.3", "cipher": TLS_CIPHER, "openssl_config": "benchmarks/openssl.cnf",
                                    "sha256": hashlib.sha256(TLS_CONFIG.read_bytes()).hexdigest(), "openssl": ssl.OPENSSL_VERSION,
                                    "wrk_policy_sha256": hashlib.sha256(TLS_SHIM.read_bytes()).hexdigest()} if WORKLOADS[args.workload][1] else None,
              "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "host": platform.node(), "kernel": capture("uname", "-a"),
              "cpu": capture("lscpu"), "java": capture(str(build.java_tool("java")), "-version"),
              "native_image": capture(str(build.java_tool("native-image")), "--version"),
              "wrk": subprocess.run(["wrk", "--version"], capture_output=True, text=True).stdout.splitlines()[0],
              "background_processes": capture("ps", "-eo", "pid,pcpu,psr,comm", "--sort=-pcpu"),
              "build_flags": {name: os.environ.get(name, "") for name in
                              ("RUSTFLAGS", "CFLAGS", "CXXFLAGS", "BEMO_BENCH_NATIVE_MARCH")},
              "artifact_sha256": fingerprint(artifacts),
              "harness_sha256": fingerprint([Path(__file__), Path(examples.__file__), Path(build.__file__),
                                              build.ROOT / "benchmarks/framework.lua", TLS_CONFIG, build.ROOT / "benchmarks/tls-policy.c", build.ROOT / "tools/versions.json"])}
  (output / "environment.json").write_text(json.dumps(metadata, indent=2) + "\n")
  manifest = build.ROOT / "source-manifest.json"
  if manifest.is_file():
    (output / "source-manifest.json").write_bytes(manifest.read_bytes())
  samples = []
  for repetition in range(1 if args.probe else args.samples):
    # Alternate which transport runs first and rotate framework/runtime pairs.
    pairs = [cases[i:i + 2] for i in range(0, len(cases), 2)]
    pairs = pairs[repetition % len(pairs):] + pairs[:repetition % len(pairs)]
    order = [case for pair in pairs for case in (pair if repetition % 2 == 0 else reversed(pair))]
    for case in order:
      samples.append(measure(args, case, repetition, output))
      (output / "samples.json").write_text(json.dumps(samples, indent=2) + "\n")
  if not args.probe:
    summary = []
    for case in cases:
      selected = [s for s in samples if (s["framework"], s["runtime"], s["transport"]) == case]
      summary.append(dict(zip(("framework", "runtime", "transport"), case),
                          driver=selected[0].get("driver"),
                          server_channel=selected[0].get("server_channel"),
                          tls_provider=selected[0]["tls_provider"],
                          tls_implementation=selected[0].get("tls_implementation"),
                          samples=len(selected),
                          median_requests_per_second=statistics.median(s["requests_per_second"] for s in selected),
                          min_requests_per_second=min(s["requests_per_second"] for s in selected),
                          max_requests_per_second=max(s["requests_per_second"] for s in selected),
                          median_latency_p50_us=statistics.median(s["latency_p50_us"] for s in selected),
                          median_latency_p99_us=statistics.median(s["latency_p99_us"] for s in selected),
                          median_server_rss_bytes=statistics.median(s["server_after"]["rss_bytes"] for s in selected),
                          median_startup_ms=statistics.median(s["startup_to_verified_http_ms"] for s in selected)))
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
  main()
