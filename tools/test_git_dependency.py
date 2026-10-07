#!/usr/bin/env python3
"""Exercise workspace inheritance as an external, revision-pinned Cargo Git consumer."""
from pathlib import Path
import shutil
import json
import os
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def run(*args, cwd):
  return subprocess.check_output(args, cwd=cwd, text=True).strip()


with tempfile.TemporaryDirectory(prefix="bemo-cargo-consumer-") as tmp:
  root = Path(tmp)
  snapshot = root / "bemo"
  snapshot.mkdir()
  for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
    shutil.copy2(ROOT / name, snapshot / name)
  shutil.copytree(ROOT / "crates", snapshot / "crates")
  shutil.copytree(ROOT / "include", snapshot / "include")
  run("git", "init", "-q", cwd=snapshot)
  run("git", "add", ".", cwd=snapshot)
  run("git", "-c", "user.name=Bemo Test", "-c", "user.email=test@example.invalid",
      "-c", "commit.gpgsign=false", "commit", "-qm", "Consumer test snapshot", cwd=snapshot)
  revision = run("git", "rev-parse", "HEAD", cwd=snapshot)
  consumer = root / "consumer"
  (consumer / "src").mkdir(parents=True)
  # A distinct workspace proves this does not inherit root-only Cargo patches.
  (consumer / "Cargo.toml").write_text(f'''[package]
name = "bemo-consumer-test"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
bemo = {{ git = "{snapshot.as_uri()}", rev = "{revision}" }}
bemo-ffi = {{ git = "{snapshot.as_uri()}", rev = "{revision}" }}
''')
  shutil.copy2(ROOT / "rust-toolchain.toml", consumer / "rust-toolchain.toml")
  (consumer / "build.rs").write_text('''fn main() {
  let directory = std::env::var("DEP_BEMO_INCLUDE").expect("Bemo shared header metadata");
  let header = std::fs::read_to_string(std::path::Path::new(&directory).join("elide_transport.h")).unwrap();
  assert!(header.contains("elide_transport_driver_poll("));
  assert!(header.contains("elide_transport_socket_receive_new_result("));
  assert!(std::path::Path::new(&directory).join("bemo.h").is_file());
}
''')
  (consumer / "src/main.rs").write_text('''unsafe extern "C" {
  fn elide_transport_buffer_release(buffer: u64) -> i32;
}
fn main() {
  assert_eq!(bemo::ABI_VERSION, bemo_ffi::bemo_abi_version());
  assert_eq!(bemo::CAPABILITIES, bemo_ffi::bemo_capabilities());
  let owner = bemo::abi::elide_transport_owner_new(4096);
  assert_ne!(owner, 0);
  let buffer = bemo::abi::elide_transport_buffer_new(owner, 128);
  assert_ne!(buffer, 0);
  // The native export and Rust facade must share exactly one handle registry.
  assert_eq!(unsafe { elide_transport_buffer_release(buffer) }, 0);
  assert_eq!(bemo::abi::elide_transport_owner_used(owner), 0);
  assert_eq!(bemo::abi::elide_transport_owner_release(owner), 0);
}
''')
  run("cargo", "generate-lockfile", cwd=consumer)
  metadata = json.loads(run("cargo", "metadata", "--format-version", "1", "--locked", cwd=consumer))
  for name in ("compio-buf", "compio-driver", "compio-log", "polling"):
    packages = [p for p in metadata["packages"] if p["name"] == name]
    assert len(packages) == 1, f"Split dependency graph for {name}"
    assert packages[0]["source"].startswith("git+https://github.com/elide-tools/"), packages[0]
  compression = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["dependencies"]["zlib-rs"]
  providers = [p for p in metadata["packages"] if p["name"] == "zlib-rs"]
  assert len(providers) == 1, "Split compression provider graph"
  assert providers[0]["source"] == f"git+{compression['git']}?rev={compression['rev']}#{compression['rev']}", providers[0]
  subprocess.run(["cargo", "run", "--locked"], cwd=consumer, check=True,
                 env={**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target/consumer-check")})
  print("External Cargo Git dependency contract passed")
