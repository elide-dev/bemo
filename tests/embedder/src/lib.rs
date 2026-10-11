//! A static library that re-exports Bemo's C boundary next to the embedder's own symbols.

use std::sync::atomic::{AtomicUsize, Ordering};

// Linking the crate keeps every `bemo_*` and `elide_transport_*` export in this archive.
pub use bemo_ffi;
// The embedder's own mimalloc supplies the mi_malloc/mi_free symbols Bemo calls.
#[cfg(feature = "embedder-mimalloc")]
pub use libmimalloc_sys;

static LIVE: AtomicUsize = AtomicUsize::new(0);

/// The embedder's allocator: the system allocator plus a live-byte count, so the C program can
/// prove Bemo's buffers were allocated here under `rust-allocator`.
struct Counting;

// SAFETY: Delegates to the system allocator with the caller's layout unchanged.
unsafe impl std::alloc::GlobalAlloc for Counting {
  unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
    // SAFETY: Forwarded with the same nonzero layout.
    let pointer = unsafe { std::alloc::System.alloc(layout) };
    if !pointer.is_null() {
      LIVE.fetch_add(layout.size(), Ordering::Relaxed);
    }
    pointer
  }

  unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
    LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    // SAFETY: The pointer came from `alloc` above with this layout.
    unsafe { std::alloc::System.dealloc(pointer, layout) };
  }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Bytes currently allocated through the embedder's Rust allocator.
#[unsafe(no_mangle)]
pub extern "C" fn embedder_live_bytes() -> u64 {
  LIVE.load(Ordering::Relaxed) as u64
}
