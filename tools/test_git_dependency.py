#!/usr/bin/env python3
"""Exercise workspace inheritance as an external, revision-pinned Cargo Git consumer."""
from pathlib import Path
import shutil
import json
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(*args, cwd):
  return subprocess.check_output(args, cwd=cwd, text=True).strip()


with tempfile.TemporaryDirectory(prefix="dokar-cargo-consumer-") as tmp:
  root = Path(tmp)
  snapshot = root / "dokar"
  snapshot.mkdir()
  for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
    shutil.copy2(ROOT / name, snapshot / name)
  shutil.copytree(ROOT / "crates", snapshot / "crates")
  shutil.copytree(ROOT / "include", snapshot / "include")
  run("git", "init", "-q", cwd=snapshot)
  run("git", "add", ".", cwd=snapshot)
  run("git", "-c", "user.name=Dokar Test", "-c", "user.email=test@example.invalid",
      "-c", "commit.gpgsign=false", "commit", "-qm", "Consumer test snapshot", cwd=snapshot)
  revision = run("git", "rev-parse", "HEAD", cwd=snapshot)
  consumer = root / "consumer"
  (consumer / "src").mkdir(parents=True)
  # A distinct workspace proves this does not inherit root-only Cargo patches.
  (consumer / "Cargo.toml").write_text(f'''[package]
name = "dokar-consumer-test"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
dokar = {{ git = "{snapshot.as_uri()}", rev = "{revision}" }}
dokar-ffi = {{ git = "{snapshot.as_uri()}", rev = "{revision}" }}
''')
  shutil.copy2(ROOT / "rust-toolchain.toml", consumer / "rust-toolchain.toml")
  (consumer / "src/main.rs").write_text('''unsafe extern "C" {
  fn elide_transport_buffer_release(buffer: u64) -> i32;
}
fn main() {
  assert_eq!(dokar::ABI_VERSION, dokar_ffi::dokar_abi_version());
  assert_eq!(dokar::CAPABILITIES, dokar_ffi::dokar_capabilities());
  let owner = dokar::abi::elide_transport_owner_new(4096);
  assert_ne!(owner, 0);
  let buffer = dokar::abi::elide_transport_buffer_new(owner, 128);
  assert_ne!(buffer, 0);
  // The native export and Rust facade must share exactly one handle registry.
  assert_eq!(unsafe { elide_transport_buffer_release(buffer) }, 0);
  assert_eq!(dokar::abi::elide_transport_owner_used(owner), 0);
  assert_eq!(dokar::abi::elide_transport_owner_release(owner), 0);
}
''')
  run("cargo", "generate-lockfile", cwd=consumer)
  metadata = json.loads(run("cargo", "metadata", "--format-version", "1", "--locked", cwd=consumer))
  for name in ("compio-buf", "compio-driver", "compio-log", "polling"):
    packages = [p for p in metadata["packages"] if p["name"] == name]
    assert len(packages) == 1, f"Split dependency graph for {name}"
    assert packages[0]["source"].startswith("git+https://github.com/elide-tools/"), packages[0]
  subprocess.run(["cargo", "run", "--locked"], cwd=consumer, check=True,
                 env={**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target/consumer-check")})
  print("External Cargo Git dependency contract passed")
