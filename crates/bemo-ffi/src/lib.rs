//! The single C boundary consumed by FFM and Native Image.
//! No JVM, JNI, GraalVM, or Elide runtime dependency belongs here.

#![deny(missing_docs)]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("Bemo's ABI requires a 64-bit target");

use std::ffi::c_void;

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

/// Allocate `size` uninitialized bytes, aligned to at least 16, from the transport's allocator.
/// Returns null for zero, sizes over 2^31 - 1, or allocation failure. Any thread may call this.
#[unsafe(no_mangle)]
pub extern "C" fn bemo_alloc(size: u64) -> *mut c_void {
  bemo::abi::bemo_alloc(size)
}

/// Free memory returned by `bemo_alloc` with the same size; null is ignored.
///
/// # Safety
/// `pointer` must come from `bemo_alloc` with this `size`, be freed once, and not be used after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bemo_free(pointer: *mut c_void, size: u64) {
  // SAFETY: The caller upholds bemo_free's contract, which bemo::abi::bemo_free shares.
  unsafe { bemo::abi::bemo_free(pointer, size) }
}

/// Bytes currently allocated through `bemo_alloc` and not yet freed, process-wide.
#[unsafe(no_mangle)]
pub extern "C" fn bemo_alloc_live() -> u64 {
  bemo::abi::bemo_alloc_live()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn boundary_matches_core() {
    assert_eq!(bemo_abi_version(), bemo::ABI_VERSION);
    assert_eq!(bemo_capabilities(), bemo::CAP_TRANSPORT_V3 | bemo::CAP_NATIVE_ALLOC);
  }
}

mod transport;
