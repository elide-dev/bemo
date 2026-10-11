#!/usr/bin/env python3
"""Bounded native fuzzing, end-to-end sanitizer tests, and scoped Miri verification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import platform
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("http", "buffers", "abi", "gzip", "response", "tls", "h2")


def run(command, env, directory, name, timeout):
  print("+ " + " ".join(map(str, command)), flush=True)
  with open(directory / f"{name}.log", "w") as log:
    subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                   check=True, timeout=timeout)


SANITIZER_FEATURES = ["--no-default-features", "--features", "bemo/rust-allocator,bemo-ffi/rust-allocator"]


def sanitize(mode, host, env, directory, manifest, stages):
  """Instrument Rust, Rust std, AWS-LC, and C consumers with one Elide toolchain runtime."""
  import setup_llvm
  home = setup_llvm.home((mode,))
  toolchain = setup_llvm.environment(home, host, mode)
  env.pop("RUSTFLAGS", None)
  env.update(toolchain)
  key = "CARGO_TARGET_" + host.upper().replace("-", "_") + "_RUSTFLAGS"
  # The toolchain flags select clang's runtime (-Zexternal-clangrt), linked by its wrapper.
  # rustc links with -nodefaultlibs, and the gnu libgcc_s script resolves only to libunwind;
  # TSAN's unwinding cleanups need __gcc_personality_v0 from compiler-rt builtins.
  builtins = subprocess.check_output([toolchain["CC"], "-print-libgcc-file-name"], text=True).strip()
  env[key] = toolchain[key] + f" -Cdebuginfo=2 -Cforce-frame-pointers=yes -Clink-arg={builtins}"
  env["ASAN_OPTIONS"] = "halt_on_error=1:abort_on_error=1:detect_leaks=" + ("1" if "linux" in host else "0")
  env["TSAN_OPTIONS"] = "halt_on_error=1:exitcode=88:second_deadlock_stack=1"
  addon = home / f"share/elide-toolchain/sanitizers/{mode}.addon.json"
  manifest.update(toolchain=(home / "share/elide-toolchain/VERSION").read_text().strip(),
                  addon=addon.is_file(), external_crypto_instrumented=True,
                  instrumented=["rust", "rust-std", "aws-lc", "c-consumers"] + (["libc++"] if addon.is_file() else []),
                  stages=stages)
  (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
  cargo = ["--locked", "--target", host, "-Zbuild-std", *SANITIZER_FEATURES]
  if "rust" in stages:
    # Explicit targets keep proc macros and build scripts out of target instrumentation.
    run(["cargo", "nextest", "run", "--workspace", "--lib", "--tests", "--profile", "ci", *cargo],
        env, directory, "tests", 1800)
  if not {"native", "jvm"} & set(stages):
    return
  run(["cargo", "build", "-p", "bemo-ffi", *cargo], env, directory, "library", 1800)
  output = Path(env["CARGO_TARGET_DIR"]) / host / "debug"
  shared = output / {"Darwin": "libbemo_ffi.dylib"}.get(platform.system(), "libbemo_ffi.so")
  # One shared runtime for the JVM, its preloaded library, and every C consumer.
  runtime = Path(toolchain["ELIDE_SANITIZER_RUNTIME"])
  cc = [env["CC"], "-shared-libsan", f"-Wl,-rpath,{runtime.parent}", "-g"]
  if "native" in stages:
    binary = directory / "static-contract"
    # Debug build-std keeps unreachable f16 conversions that compiler-rt does not define.
    libraries = ["-ldl", "-lpthread", "-lm", "-Wl,--gc-sections"] if "linux" in host else []
    run([*cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-I", ROOT / "include",
         ROOT / "tests/static.c", output / "libbemo_ffi.a", *libraries, "-o", binary],
        env, directory, "static-build", 300)
    run([binary], env, directory, "static-contract", 60)
  if "jvm" in stages:
    if "linux" not in host:
      # macOS would need DYLD_INSERT_LIBRARIES into a hardened JDK; keep the Rust and C stages.
      print("Skipping sanitized JVM contracts: Linux only", flush=True)
      manifest["stages"] = [stage for stage in stages if stage != "jvm"]
      return
    import build
    from reports import Reports
    build.jvm()
    jvm = dict(env)
    jvm["LD_PRELOAD"] = str(runtime)
    # HotSpot uses SIGSEGV for safepoints and implicit null checks, and never frees
    # much of its own metadata: leave both to the JVM and keep reports fatal.
    jvm["ASAN_OPTIONS"] = "halt_on_error=1:detect_leaks=0:handle_segv=0:allow_user_segv_handler=1"
    suppressions = ROOT / f"tools/sanitizers/{mode}-jvm.supp"
    if mode == "tsan":
      jvm["TSAN_OPTIONS"] = env["TSAN_OPTIONS"] + ":ignore_noninstrumented_modules=1" + (
          f":suppressions={suppressions}" if suppressions.is_file() else "")
    canary(mode, cc, env, jvm, directory)
    reports = Reports(directory / "jvm")
    build.ffm_contracts(reports, shared, env=jvm, cc=cc)
    reports.finish()


def canary(mode, cc, env, jvm, directory):
  """Require the sanitized JVM launch to report deliberate defects before trusting it."""
  import build
  output = directory / "canary"
  output.mkdir(exist_ok=True)
  library = output / "libcanary.so"
  run([*cc, "-shared", "-fPIC", "-O1", ROOT / "tests/sanitizer/canary.c", "-o", library],
      env, directory, "canary-build", 120)
  build.compile_java(output / "classes", [ROOT / "tests/sanitizer/SanitizerCanary.java"],
                     lint="all,-restricted")
  java = os.environ.get("BEMO_TEST_JAVA", build.java_tool("java"))
  case, marker = {"asan": ("overflow", "AddressSanitizer: heap-buffer-overflow"),
                  "tsan": ("race", "ThreadSanitizer: data race")}[mode]
  log = directory / "canary.log"
  with log.open("w") as output_log:
    result = subprocess.run([java, "--enable-native-access=ALL-UNNAMED", "-cp", output / "classes",
                             "SanitizerCanary", library, case], cwd=ROOT, env=jvm, stdout=output_log,
                            stderr=subprocess.STDOUT, timeout=300)
  text = log.read_text(errors="replace")
  if result.returncode == 0 or marker not in text:
    raise RuntimeError(f"{mode} canary was not reported in the JVM; inspect {log.relative_to(ROOT)}")
  print(f"{mode} JVM canary reported: {marker}", flush=True)


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("mode", choices=("asan", "tsan", "miri", "fuzz"))
  parser.add_argument("--seconds", type=int, default=30, help="Per-fuzzer time budget")
  parser.add_argument("--runs", type=int, default=0, help="Optional fixed per-fuzzer execution budget")
  parser.add_argument("--stages", default="rust,native,jvm",
                      help="Sanitizer stages: rust (unit/integration), native (C consumers), jvm (FFM contracts)")
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
        for kind in ("seeds", "regressions"):
          for seed in sorted((ROOT / "fuzz" / kind / target).glob("*")):
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
      stages = [stage for stage in args.stages.split(",") if stage]
      if not set(stages) <= {"rust", "native", "jvm"}:
        parser.error("stages must be drawn from rust, native, jvm")
      sanitize(args.mode, host, env, directory, manifest, stages)
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
