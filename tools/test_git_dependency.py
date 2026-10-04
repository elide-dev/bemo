#!/usr/bin/env python3
"""Exercise workspace inheritance as an external, revision-pinned Cargo Git consumer."""
from pathlib import Path
import shutil
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
  (consumer / "src/main.rs").write_text('''fn main() {
  assert_eq!(dokar::ABI_VERSION, dokar_ffi::dokar_abi_version());
  assert_eq!(dokar::CAPABILITIES, dokar_ffi::dokar_capabilities());
}
''')
  run("cargo", "generate-lockfile", cwd=consumer)
  run("cargo", "run", "--locked", cwd=consumer)
  print("External Cargo Git dependency contract passed")
