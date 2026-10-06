use std::path::PathBuf;

fn main() {
  let include = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
    .join("../../include")
    .canonicalize()
    .expect("Bemo shared headers");
  println!("cargo::metadata=include={}", include.display());
  println!("cargo::rerun-if-changed={}", include.join("bemo.h").display());
  println!(
    "cargo::rerun-if-changed={}",
    include.join("elide_transport.h").display()
  );
}
