use std::path::PathBuf;

fn main() {
  if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
    // Some linker drivers omit the default ID; debug peers need a byte identity.
    println!("cargo::rustc-link-arg-cdylib=-Wl,--build-id=sha1");
  }
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
