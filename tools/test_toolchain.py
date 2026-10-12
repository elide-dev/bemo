#!/usr/bin/env python3
"""Qualify Bemo for Elide's builds with the pinned Elide toolchain.

`musl` runs the Rust suite as fully static x86_64-unknown-linux-musl executables linked by
the toolchain. `embedder` builds tests/embedder (default Bemo features off, the embedder's
own allocator) into a static library, links it into a C program with the toolchain's
clang for glibc and static musl, runs it, and inspects the archive's symbols.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import seam
import setup_llvm

ROOT = Path(__file__).resolve().parents[1]
CRATE = ROOT / "tests/embedder"

# (name, Rust target, static link, embedder features)
CASES = {
  "gnu-mimalloc": ("x86_64-unknown-linux-gnu", False, ["embedder-mimalloc"]),
  "gnu-rust-allocator": ("x86_64-unknown-linux-gnu", False, ["rust-allocator"]),
  "musl-libc-mimalloc": ("x86_64-unknown-linux-musl", True, []),
  "musl-rust-allocator": ("x86_64-unknown-linux-musl", True, ["rust-allocator"]),
}


def run(command, **kwargs):
  print("+ " + " ".join(map(str, command)), flush=True)
  return subprocess.run(list(map(str, command)), check=True, **kwargs)


def header_exports():
  text = "\n".join((ROOT / "include" / name).read_text() for name in ("bemo.h", "elide_transport.h"))
  return set(re.findall(r"\b((?:bemo|elide_transport)_[a-z0-9_]+)\s*\(", text))


def defined(archive, nm):
  output = subprocess.check_output([nm, "--defined-only", "--extern-only", "--format=just-symbols", archive],
                                   text=True, stderr=subprocess.DEVNULL)
  return output.split()


def case(name, home, target_dir):
  triple, static, features = CASES[name]
  env = dict(os.environ)
  env.pop("RUSTFLAGS", None)
  env.update(setup_llvm.environment(home, triple, static=static))
  env.pop("LDFLAGS", None)
  env["CARGO_TARGET_DIR"] = str(target_dir)
  if static:
    env["CARGO_TARGET_" + triple.upper().replace("-", "_") + "_RUSTFLAGS"] = "-Ctarget-feature=+crt-static"
  command = ["cargo", "build", "--manifest-path", CRATE / "Cargo.toml", "--locked", "--target", triple]
  if features:
    command += ["--features", ",".join(features)]
  run(command, env=env)
  archive = target_dir / triple / "debug/libbemo_embedder_test.a"
  nm = Path(home) / "bin/llvm-nm"
  symbols = defined(archive, nm)
  exports = header_exports()
  missing = sorted(exports - set(symbols))
  assert not missing, f"{name}: archive lacks header exports {missing[:5]}"
  duplicated = sorted(symbol for symbol in exports if symbols.count(symbol) != 1)
  assert not duplicated, f"{name}: exports defined more than once {duplicated[:5]}"
  bundled = "mi_malloc" in symbols
  # Only an embedder that brings its own mimalloc may carry one; Bemo never bundles it here.
  assert bundled == ("embedder-mimalloc" in features), f"{name}: unexpected mimalloc presence ({bundled})"
  binary = target_dir / f"embedder-{name}"
  flags = ["-static"] if static else []
  defines = ["-DBEMO_EMBEDDER_RUST_ALLOCATOR"] if "rust-allocator" in features else []
  libraries = [] if static else ["-ldl", "-lpthread", "-lm"]
  run([env["CC"], *flags, *defines, "-std=c11", "-Wall", "-Wextra", "-Werror", "-I", ROOT / "include",
       CRATE / "main.c", archive, *libraries, "-o", binary], env=env)
  if static:
    kind = subprocess.check_output(["file", binary], text=True)
    assert "statically linked" in kind or "static-pie linked" in kind, kind
  run([binary])
  allocator = "Rust global allocator" if "rust-allocator" in features else ("embedder mimalloc" if bundled else "libc mimalloc")
  print(f"{name}: passed ({len(exports)} exports, {allocator})", flush=True)


def musl(home, target_dir):
  triple = "x86_64-unknown-linux-musl"
  env = dict(os.environ)
  env.pop("RUSTFLAGS", None)
  env.update(setup_llvm.environment(home, triple, static=True))
  env.pop("LDFLAGS", None)
  env["CARGO_TARGET_DIR"] = str(target_dir)
  env["CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS"] = "-Ctarget-feature=+crt-static"
  # Default features: bundled mimalloc must coexist with the mimalloc in the toolchain's musl libc.
  run(["cargo", "nextest", "run", "--workspace", "--lib", "--tests", "--locked", "--profile", "ci",
       "--target", triple], env=env, cwd=ROOT)
  print("Static musl suite passed", flush=True)


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  commands = parser.add_subparsers(dest="command", required=True)
  commands.add_parser("musl", help="Rust suite as static musl executables")
  embedder = commands.add_parser("embedder", help="Elide-shaped static library embedding")
  embedder.add_argument("cases", nargs="*", choices=list(CASES))
  args = parser.parse_args()
  if seam.target_triple() != "x86_64-unknown-linux-gnu":
    print("Toolchain qualification runs on Linux x86-64 hosts; skipping")
    return
  home = setup_llvm.home()
  target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "toolchain"
  if args.command == "musl":
    musl(home, target_dir)
    return
  for name in args.cases or CASES:
    case(name, home, target_dir)
  print("Embedder contract passed: " + ", ".join(args.cases or CASES))


if __name__ == "__main__":
  main()
