//! The single C boundary consumed by FFM and Native Image.
//! No JVM, JNI, GraalVM, or Elide runtime dependency belongs here.

#![deny(missing_docs)]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("Bemo's ABI requires a 64-bit target");

/// Return the C ABI version before resolving version-specific symbols.
#[unsafe(no_mangle)]
pub extern "C" fn bemo_abi_version() -> u32 {
  bemo::ABI_VERSION
}

/// Return implemented data-plane capability bits.
#[unsafe(no_mangle)]
pub extern "C" fn bemo_capabilities() -> u64 {
  bemo::CAPABILITIES
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn boundary_matches_core() {
    assert_eq!(bemo_abi_version(), bemo::ABI_VERSION);
    assert_eq!(bemo_capabilities(), bemo::CAP_TRANSPORT_V3);
  }
}

mod transport;
