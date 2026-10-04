//! The single C boundary consumed by FFM and Native Image.
//! No JVM, JNI, GraalVM, or Elide runtime dependency belongs here.

#![deny(missing_docs)]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("Dokar's ABI requires a 64-bit target");

/// Return the C ABI version before resolving version-specific symbols.
#[unsafe(no_mangle)]
pub extern "C" fn dokar_abi_version() -> u32 {
  dokar::ABI_VERSION
}

/// Return implemented data-plane capability bits.
#[unsafe(no_mangle)]
pub extern "C" fn dokar_capabilities() -> u64 {
  dokar::CAPABILITIES
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn boundary_matches_core() {
    assert_eq!(dokar_abi_version(), dokar::ABI_VERSION);
    assert_eq!(dokar_capabilities(), dokar::CAP_TRANSPORT_V3);
  }
}

mod transport;
