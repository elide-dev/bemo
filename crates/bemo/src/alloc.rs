/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Raw allocations from the transport's own allocator, for embedder-managed memory such as
//! Netty direct buffers. These pair with exactly the allocator native buffers use: bundled
//! mimalloc, the embedder's `mi_malloc`, or the Rust global allocator (see `buffer.rs`).
//! They are not charged to any [`Budget`](crate::buffer::Budget); callers own their lifetime.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::buffer::{mi_free, mi_malloc};

/// Largest single allocation, matching the largest Java direct buffer.
pub const MAX_ALLOCATION: usize = i32::MAX as usize;

static LIVE: AtomicU64 = AtomicU64::new(0);

/// Allocate `size` bytes aligned to at least 16. Returns null for zero, sizes over
/// [`MAX_ALLOCATION`], or allocation failure. The memory is uninitialized.
pub fn allocate(size: usize) -> *mut u8 {
  if size == 0 || size > MAX_ALLOCATION {
    return std::ptr::null_mut();
  }
  // SAFETY: size is nonzero and bounded well below isize::MAX.
  let pointer = unsafe { mi_malloc(size) }.cast::<u8>();
  if !pointer.is_null() {
    LIVE.fetch_add(size as u64, Ordering::Relaxed);
  }
  pointer
}

/// Free memory from [`allocate`]. Null is ignored.
///
/// # Safety
/// `pointer` must come from [`allocate`] with this `size` and must not be used afterwards.
pub unsafe fn free(pointer: *mut u8, size: usize) {
  if pointer.is_null() {
    return;
  }
  LIVE.fetch_sub(size as u64, Ordering::Relaxed);
  // SAFETY: The caller guarantees pointer came from mi_malloc through allocate and is live.
  unsafe { mi_free(pointer.cast::<c_void>()) };
}

/// Bytes currently allocated through [`allocate`] and not yet freed, process-wide.
pub fn live() -> u64 {
  LIVE.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn allocations_are_aligned_writable_and_accounted() {
    let before = live();
    let pointer = allocate(4096);
    assert!(!pointer.is_null());
    assert_eq!(pointer as usize % 16, 0);
    // SAFETY: allocate returned 4096 writable bytes.
    unsafe { pointer.write_bytes(0xa5, 4096) };
    // SAFETY: The final byte is inside the allocation and was initialized above.
    assert_eq!(unsafe { pointer.add(4095).read() }, 0xa5);
    assert!(live() >= before + 4096);
    // SAFETY: Freed once with its allocation size.
    unsafe { free(pointer, 4096) };
  }

  #[test]
  fn invalid_sizes_and_null_frees_do_nothing() {
    assert!(allocate(0).is_null());
    assert!(allocate(MAX_ALLOCATION + 1).is_null());
    // SAFETY: Null is explicitly ignored.
    unsafe { free(std::ptr::null_mut(), 4096) };
  }
}
