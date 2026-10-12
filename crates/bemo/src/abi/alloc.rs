/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Raw allocations from the transport's allocator (`CAP_NATIVE_ALLOC`).

use std::ffi::c_void;

/// Allocate `size` uninitialized bytes, aligned to at least 16, from the transport's allocator.
/// Returns null for zero, sizes over [`crate::alloc::MAX_ALLOCATION`], or allocation failure.
/// Any thread may call this.
pub fn bemo_alloc(size: u64) -> *mut c_void {
  usize::try_from(size).map_or(std::ptr::null_mut(), |size| crate::alloc::allocate(size).cast())
}

/// Free memory returned by [`bemo_alloc`]; null is ignored. Any thread may call this.
///
/// # Safety
/// `pointer` must come from [`bemo_alloc`] with the same `size`, be freed once, and not be
/// accessed afterwards.
pub unsafe fn bemo_free(pointer: *mut c_void, size: u64) {
  // SAFETY: Forwarded under the caller's contract; a size that did not fit was never allocated.
  unsafe { crate::alloc::free(pointer.cast(), usize::try_from(size).unwrap_or(0)) };
}

/// Bytes currently allocated through [`bemo_alloc`] and not yet freed, process-wide.
pub fn bemo_alloc_live() -> u64 {
  crate::alloc::live()
}
