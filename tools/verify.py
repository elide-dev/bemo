#!/usr/bin/env python3
"""Bounded native fuzzing, sanitizer tests, and scoped Miri verification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("http", "buffers", "abi")


def run(command, env, directory, name, timeout):
  print("+ " + " ".join(map(str, command)), flush=True)
  with open(directory / f"{name}.log", "w") as log:
    subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                   check=True, timeout=timeout)


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("mode", choices=("asan", "tsan", "miri", "fuzz"))
  parser.add_argument("--seconds", type=int, default=30, help="Per-fuzzer time budget")
  parser.add_argument("--runs", type=int, default=0, help="Optional fixed per-fuzzer execution budget")
  args = parser.parse_args()
  if args.seconds < 1 or args.runs < 0:
    parser.error("seconds must be positive and runs nonnegative")
  host = next(line.split(": ", 1)[1] for line in subprocess.check_output(
      ["rustc", "-vV"], cwd=ROOT, text=True).splitlines() if line.startswith("host: "))
  env = dict(os.environ)
  directory = ROOT / "build/reports/verification" / args.mode
  shutil.rmtree(directory, ignore_errors=True)
  directory.mkdir(parents=True)
  manifest = {"mode": args.mode, "target": host, "status": "running",
              "rustc": subprocess.check_output(["rustc", "-vV"], cwd=ROOT, text=True),
              "allocator": "rust", "external_crypto_instrumented": False}
  (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
  env["CARGO_TARGET_DIR"] = str(ROOT / "target/verification" / args.mode)
  try:
    if args.mode == "fuzz":
      versions = json.loads((ROOT / "tools/versions.json").read_text())
      actual = subprocess.check_output(["cargo", "fuzz", "--version"], cwd=ROOT, text=True).strip()
      if actual != "cargo-fuzz " + versions["fuzz"]:
        raise RuntimeError("Install cargo-fuzz " + versions["fuzz"])
      lock = ROOT / "fuzz/Cargo.lock"
      lock_hash = hashlib.sha256(lock.read_bytes()).hexdigest()
      run(["cargo", "metadata", "--manifest-path", "fuzz/Cargo.toml", "--locked",
           "--format-version", "1", "--no-deps"], env, directory, "dependencies", 120)
      for target in TARGETS:
        corpus = ROOT / "fuzz/corpus" / target
        corpus.mkdir(parents=True, exist_ok=True)
        for seed in (ROOT / "fuzz/seeds" / target).iterdir():
          shutil.copy2(seed, corpus / seed.name)
        artifacts = ROOT / "fuzz/artifacts" / target
        artifacts.mkdir(parents=True, exist_ok=True)
        command = ["cargo", "fuzz", "run", "--target", host, target, str(corpus), "--",
                   "-seed=1", "-max_len=32768", "-timeout=5", "-rss_limit_mb=2048",
                   f"-artifact_prefix={artifacts}{os.sep}"]
        command.append(f"-runs={args.runs}" if args.runs else f"-max_total_time={args.seconds}")
        run(command, env, directory, target, args.seconds + 900)
        if hashlib.sha256(lock.read_bytes()).hexdigest() != lock_hash:
          raise RuntimeError("Fuzz dependencies changed; review and commit the lockfile")
    elif args.mode == "miri":
      env["MIRIFLAGS"] = "-Zmiri-disable-isolation"
      run(["cargo", "miri", "test", "-p", "bemo", "--locked", "--no-default-features",
           "--features", "rust-allocator", "--lib", "--test", "buffers", "--test", "abi",
           "--test", "pool", "--test", "workload"], env, directory, "tests", 1800)
    else:
      sanitizer = "address" if args.mode == "asan" else "thread"
      env["RUSTFLAGS"] = f"-Zsanitizer={sanitizer} -Cdebuginfo=2 -Cforce-frame-pointers=yes"
      env["ASAN_OPTIONS"] = "halt_on_error=1:abort_on_error=1:detect_leaks=" + ("1" if "linux" in host else "0")
      env["TSAN_OPTIONS"] = "halt_on_error=1:exitcode=88"
      # Explicit targets keep proc macros and build scripts out of target instrumentation.
      run(["cargo", "nextest", "run", "--workspace", "--lib", "--tests", "--locked",
           "--profile", "ci", "--target", host, "-Zbuild-std", "--no-default-features",
           "--features", "bemo/rust-allocator,bemo-ffi/rust-allocator"],
          env, directory, "tests", 1800)
    manifest["status"] = "passed"
  except BaseException:
    manifest["status"] = "failed"
    print(f"Verification failed; inspect {directory.relative_to(ROOT)}", file=sys.stderr)
    raise
  finally:
    (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
  print(f"{args.mode}: passed; evidence in {directory.relative_to(ROOT)}")


if __name__ == "__main__":
  main()
