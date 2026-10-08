/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Owned native storage. Mutable buffers cannot be cloned; frozen views retain their allocation.

pub(crate) mod provided;
pub use provided::ReceiveCredit;

use std::cell::RefCell;
use std::ffi::c_void;
use std::io;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut, Range};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use compio_buf::{IoBuf, IoBufMut, SetLen};

#[cfg(all(
  feature = "bundled-mimalloc",
  feature = "rust-allocator",
  not(miri),
  not(target_vendor = "apple")
))]
compile_error!("Select either bundled-mimalloc or rust-allocator, not both");

#[cfg(all(
  feature = "bundled-mimalloc",
  not(feature = "rust-allocator"),
  not(miri),
  not(target_vendor = "apple")
))]
use libmimalloc_sys::{mi_free, mi_malloc};

#[cfg(all(
  not(feature = "bundled-mimalloc"),
  not(feature = "rust-allocator"),
  not(miri),
  not(target_vendor = "apple")
))]
unsafe extern "C" {
  fn mi_malloc(size: usize) -> *mut c_void;
  fn mi_free(pointer: *mut c_void);
}

#[cfg(any(miri, target_vendor = "apple", feature = "rust-allocator"))]
use rust_alloc::{mi_free, mi_malloc};

// The bundled allocator is still linked under miri, just never called from here.
#[cfg(all(feature = "bundled-mimalloc", miri, not(target_vendor = "apple")))]
use libmimalloc_sys as _;

/// Stands mimalloc up on the Rust allocator: Miri cannot call into mimalloc, and on Apple a second
/// mimalloc in the engine dylib shares the executable's fixed TLS heap slot, so blocks cross
/// instances and the host interposer frees them as foreign. `mi_free` takes no size, so the layout
/// rides in a header ahead of the returned pointer; the header is `MI_ALIGN` wide to keep payload
/// alignment at mimalloc's guarantee.
#[cfg(any(miri, target_vendor = "apple", feature = "rust-allocator"))]
mod rust_alloc {
  use std::alloc::{Layout, alloc, dealloc};
  use std::ffi::c_void;

  const MI_ALIGN: usize = 16;

  fn layout(size: usize) -> Layout {
    // Checked by `Buffer::allocate`, which rejects zero and anything past `isize::MAX`.
    Layout::from_size_align(size + MI_ALIGN, MI_ALIGN).expect("allocation size out of range")
  }

  pub(super) unsafe fn mi_malloc(size: usize) -> *mut c_void {
    // SAFETY: layout returns a nonzero valid layout, including header space and MI_ALIGN alignment.
    let base = unsafe { alloc(layout(size)) };
    if base.is_null() {
      return std::ptr::null_mut();
    }
    // SAFETY: The allocation includes a usize-sized header; unaligned access avoids strengthening the pointer type.
    unsafe { base.cast::<usize>().write_unaligned(size) };
    // SAFETY: The allocation includes MI_ALIGN header bytes before its payload.
    unsafe { base.add(MI_ALIGN) }.cast::<c_void>()
  }

  pub(super) unsafe fn mi_free(pointer: *mut c_void) {
    if pointer.is_null() {
      return;
    }
    // SAFETY: pointer was returned by mi_malloc; its allocation includes the preceding header.
    let base = unsafe { pointer.cast::<u8>().sub(MI_ALIGN) };
    // SAFETY: mi_malloc initialized this header; use an unaligned read from the byte pointer.
    let size = unsafe { base.cast::<usize>().read_unaligned() };
    // SAFETY: base and the recovered layout match the original alloc call exactly.
    unsafe { dealloc(base, layout(size)) };
  }
}

/// Allocation capacity charged to a resource owner, including retained slices.
#[derive(Clone, Debug)]
pub struct Budget(Arc<BudgetInner>);

#[derive(Debug)]
struct BudgetInner {
  limit: usize,
  used: AtomicUsize,
  closed: AtomicBool,
  workload: AtomicBool,
}

impl Budget {
  /// Create a bounded owner. Zero forbids allocation.
  pub fn new(limit: usize) -> Self {
    Self(Arc::new(BudgetInner {
      limit,
      used: AtomicUsize::new(0),
      closed: AtomicBool::new(false),
      workload: AtomicBool::new(false),
    }))
  }

  #[cfg(target_os = "linux")]
  pub(crate) fn identity(&self) -> usize {
    Arc::as_ptr(&self.0) as usize
  }

  /// Capacity retained by live allocations. Pooled storage is not live and is excluded.
  pub fn used(&self) -> usize {
    self.0.used.load(Ordering::Acquire)
  }

  /// Stop new allocations through every clone; existing allocations remain valid.
  pub fn close(&self) {
    self.0.closed.store(true, Ordering::SeqCst);
  }

  /// Whether admission has stopped; a closed owner never reopens.
  pub fn is_closed(&self) -> bool {
    self.0.closed.load(Ordering::SeqCst)
  }

  /// Record that this owner names work, then report whether it still admits any. Sequentially
  /// consistent with [`Self::close`] and [`Self::is_workload`]: a racing closer either sees the mark
  /// or this call sees the closure.
  pub(crate) fn admit_work(&self) -> bool {
    self.0.workload.store(true, Ordering::SeqCst);
    !self.is_closed()
  }

  /// Whether this owner ever named work, so its closure has operations to cancel.
  pub(crate) fn is_workload(&self) -> bool {
    self.0.workload.load(Ordering::SeqCst)
  }

  fn reserve(&self, capacity: usize) -> io::Result<()> {
    if self.is_closed() {
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    self
      .0
      .used
      .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
        used.checked_add(capacity).filter(|next| *next <= self.0.limit)
      })
      .map(|_| ())
      .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    // Closure may race the charge; only publish admission after checking it again.
    if self.is_closed() {
      self.0.used.fetch_sub(capacity, Ordering::AcqRel);
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    Ok(())
  }
}

#[derive(Debug)]
struct Allocation {
  pointer: NonNull<u8>,
  capacity: usize,
  budget: Budget,
  recyclable: Option<Reuse>,
  charged: bool,
  return_slot: Option<Arc<provided::ReturnSlot>>,
  receive_credit: Option<provided::ReceiveCredit>,
}

// Mutable Buffer regions never overlap; FrozenBuffer shares only immutable regions.
// SAFETY: Mutable Buffer regions never overlap; FrozenBuffer shares only immutable regions.
unsafe impl Send for Allocation {}
// SAFETY: Shared allocations expose mutation only through disjoint, exclusively owned Buffer regions.
unsafe impl Sync for Allocation {}

impl Drop for Allocation {
  fn drop(&mut self) {
    if let Some(slot) = &self.return_slot {
      slot.release(self.pointer);
      return;
    }
    // SAFETY: The final owner releases the mi_malloc allocation exactly once after pool return was declined.
    unsafe { mi_free(self.pointer.as_ptr().cast::<c_void>()) };
    if self.charged {
      self.budget.0.used.fetch_sub(self.capacity, Ordering::AcqRel);
    }
  }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reuse {
  Exact,
  Receive,
  TlsOutput,
}

/// Maximum pooled allocations, and the retained bytes they may hold, per thread.
const POOL_ENTRIES: usize = 16;
const POOL_BYTES: usize = 256 * 1024;

/// One reusable allocation. Pooling releases its budget charge and reuse re-reserves it, so
/// `Budget::used` keeps reporting only storage a caller can still reach. The pool is bounded
/// separately, by entry count, retained bytes, and a fraction of the owner's limit.
struct Pooled {
  allocation: Arc<Allocation>,
}

/// Shares allocation metadata with live views; only its final lease can enter the idle pool.
#[derive(Debug)]
struct AllocationLease(ManuallyDrop<Arc<Allocation>>);

impl AllocationLease {
  fn new(allocation: Arc<Allocation>) -> Self {
    Self(ManuallyDrop::new(allocation))
  }
}

impl Clone for AllocationLease {
  fn clone(&self) -> Self {
    Self::new(Arc::clone(&self.0))
  }
}

impl Deref for AllocationLease {
  type Target = Arc<Allocation>;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl DerefMut for AllocationLease {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}

impl Drop for AllocationLease {
  #[inline]
  fn drop(&mut self) {
    // SAFETY: Drop transfers this Arc once; the field never drops automatically or is accessed again.
    let allocation = unsafe { ManuallyDrop::take(&mut self.0) };
    // No weak references exist. A missed admission during concurrent drops is safe: Arc frees it.
    if Arc::strong_count(&allocation) == 1 && allocation.recyclable.is_some() && allocation.return_slot.is_none() {
      Pool::admit(Pooled { allocation });
    }
  }
}

#[derive(Default)]
struct Pool {
  entries: Vec<Pooled>,
  retained: usize,
}

thread_local! {
  static POOL: RefCell<Pool> = RefCell::new(Pool::default());
}

impl Pool {
  fn discard(entry: Pooled) {
    drop(entry);
  }

  /// Retain exclusively owned metadata and storage, excluding them from live budget charges.
  fn admit(mut entry: Pooled) {
    let allocation = Arc::get_mut(&mut entry.allocation).unwrap();
    // Idle storage owns no delivery credit. External clones keep their independent reservation.
    drop(allocation.receive_credit.take());
    if allocation.capacity > POOL_BYTES || allocation.budget.is_closed() {
      return;
    }
    let _ = POOL.try_with(|pool| {
      let mut pool = pool.borrow_mut();
      let allocation = Arc::get_mut(&mut entry.allocation).unwrap();
      let fraction = if allocation.recyclable == Some(Reuse::Receive) {
        8
      } else {
        2
      };
      let room = allocation.budget.0.limit / fraction;
      let owned: usize = pool
        .entries
        .iter()
        .filter(|entry| Arc::ptr_eq(&entry.allocation.budget.0, &allocation.budget.0))
        .map(|entry| entry.allocation.capacity)
        .sum();
      if pool.entries.len() >= POOL_ENTRIES
        || pool.retained + allocation.capacity > POOL_BYTES
        || owned + allocation.capacity > room
      {
        return;
      }
      pool.retained += allocation.capacity;
      allocation
        .budget
        .0
        .used
        .fetch_sub(allocation.capacity, Ordering::AcqRel);
      allocation.charged = false;
      pool.entries.push(entry);
    });
  }

  fn take(capacity: usize, budget: &Budget, reuse: Reuse) -> Option<Arc<Allocation>> {
    POOL
      .try_with(|pool| {
        let mut pool = pool.borrow_mut();
        let index = match reuse {
          // Equal geometry needs no smallest-fit scan. Recently returned storage is usually
          // at the end; taking it there also avoids moving another entry with swap_remove.
          Reuse::Exact | Reuse::TlsOutput => pool
            .entries
            .iter()
            .rposition(|entry| entry.allocation.recyclable == Some(reuse) && entry.allocation.capacity == capacity),
          Reuse::Receive => pool
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
              let allocation = &entry.allocation;
              allocation.recyclable == Some(Reuse::Receive)
                && allocation.capacity >= capacity
                && Arc::ptr_eq(&allocation.budget.0, &budget.0)
            })
            .min_by_key(|(_, entry)| entry.allocation.capacity)
            .map(|(index, _)| index),
        }?;
        let entry = pool.entries.swap_remove(index);
        pool.retained -= entry.allocation.capacity;
        Some(entry)
      })
      .ok()
      .flatten()
      .and_then(|mut entry| {
        let allocation = Arc::get_mut(&mut entry.allocation).unwrap();
        budget.reserve(allocation.capacity).ok()?;
        if !Arc::ptr_eq(&allocation.budget.0, &budget.0) {
          allocation.budget = budget.clone();
        }
        allocation.charged = true;
        Some(entry.allocation)
      })
  }
}

impl Drop for Pool {
  fn drop(&mut self) {
    self.retained = 0;
    for entry in std::mem::take(&mut self.entries) {
      Self::discard(entry);
    }
  }
}

/// Pooled allocation capacity retained by this thread, outside every owner budget.
pub fn pool_retained() -> usize {
  POOL.try_with(|pool| pool.borrow().retained).unwrap_or(0)
}

/// Drop one idle pooled allocation, preferring storage whose owner has been released.
pub fn pool_trim() {
  let _ = POOL.try_with(|pool| {
    let mut pool = pool.borrow_mut();
    let closed = pool
      .entries
      .iter()
      .position(|entry| entry.allocation.budget.is_closed());
    let Some(index) = closed.or_else(|| pool.entries.len().checked_sub(1)) else {
      return;
    };
    let entry = pool.entries.swap_remove(index);
    pool.retained -= entry.allocation.capacity;
    Pool::discard(entry);
  });
}

/// Exclusive mutable storage suitable for completion-based receives.
#[derive(Debug)]
pub struct Buffer {
  allocation: AllocationLease,
  offset: usize,
  capacity: usize,
  length: usize,
}

const RESPONSE_SLAB: usize = 4096;
const SMALL_RESPONSE: usize = 1024;

struct ResponseArena {
  depth: usize,
  tail: Option<Buffer>,
}

thread_local! {
  static RESPONSES: RefCell<ResponseArena> = const {
    RefCell::new(ResponseArena { depth: 0, tail: None })
  };
}

/// A callback scope may share small response allocations; no spare storage survives its exit.
pub(crate) struct ResponseBatch(std::marker::PhantomData<std::rc::Rc<()>>);

impl ResponseBatch {
  pub(crate) fn enter() -> Self {
    RESPONSES.with(|arena| arena.borrow_mut().depth += 1);
    Self(std::marker::PhantomData)
  }
}

impl Drop for ResponseBatch {
  fn drop(&mut self) {
    RESPONSES.with(|arena| {
      let mut arena = arena.borrow_mut();
      arena.depth -= 1;
      if arena.depth == 0 {
        arena.tail = None;
      }
    });
  }
}

impl Buffer {
  /// Shared read-ahead charge for provided receives, independent of storage lifetime.
  pub(crate) fn receive_credit(&self) -> Option<provided::ReceiveCredit> {
    self.allocation.receive_credit.clone()
  }

  /// Allocate uninitialized native storage.
  ///
  /// # Errors
  /// Rejects zero/oversized allocations, exhausted budgets, and allocator failure.
  pub fn new(capacity: usize, budget: Budget) -> io::Result<Self> {
    if capacity > POOL_BYTES || capacity > budget.0.limit / 2 {
      return Self::allocate(capacity, budget, None);
    }
    Self::reuse(capacity, budget, Reuse::Exact)
  }

  /// Carve disjoint small responses within a callback batch; charge the entire backing slab.
  pub(crate) fn response(capacity: usize, budget: &Budget) -> io::Result<Self> {
    if capacity == 0 || capacity > SMALL_RESPONSE {
      return Self::new(capacity, budget.clone());
    }
    RESPONSES.with(|arena| {
      let mut arena = arena.borrow_mut();
      if arena.depth == 0 {
        return Self::new(capacity, budget.clone());
      }
      if budget.0.closed.load(Ordering::Acquire) {
        return Err(io::ErrorKind::BrokenPipe.into());
      }
      let tail = arena
        .tail
        .take()
        .filter(|tail| tail.capacity >= capacity && Arc::ptr_eq(&tail.allocation.budget.0, &budget.0));
      let storage = match tail {
        Some(tail) => tail,
        None => match Self::new(RESPONSE_SLAB, budget.clone()) {
          Ok(slab) => slab,
          // A small owner or a nearly exhausted budget can still afford this exact response.
          Err(_) => return Self::new(capacity, budget.clone()),
        },
      };
      let (response, tail) = storage.split_at(capacity).expect("response fits slab");
      if tail.capacity != 0 {
        arena.tail = Some(tail);
      }
      Ok(response)
    })
  }

  /// Allocate receive storage, reusing a pooled allocation of this owner when one fits.
  ///
  /// # Errors
  /// Rejects zero/oversized allocations, exhausted budgets, and allocator failure.
  pub fn receive(capacity: usize, budget: Budget) -> io::Result<Self> {
    Self::reuse(capacity, budget, Reuse::Receive)
  }

  fn reuse(capacity: usize, budget: Budget, reuse: Reuse) -> io::Result<Self> {
    if capacity == 0 || capacity > isize::MAX as usize {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    match Pool::take(capacity, &budget, reuse) {
      // The pooled capacity is already charged, so ownership transfers without a new reservation.
      Some(allocation) => Ok(Self {
        allocation: AllocationLease::new(allocation),
        offset: 0,
        capacity,
        length: 0,
      }),
      None => Self::allocate(capacity, budget, Some(reuse)),
    }
  }

  fn allocate(capacity: usize, budget: Budget, recyclable: Option<Reuse>) -> io::Result<Self> {
    if capacity == 0 || capacity > isize::MAX as usize {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    budget.reserve(capacity)?;
    // SAFETY: capacity is nonzero and within isize::MAX; the paired free retains this allocator identity.
    let pointer = NonNull::new(unsafe { mi_malloc(capacity) }.cast::<u8>());
    let Some(pointer) = pointer else {
      budget.0.used.fetch_sub(capacity, Ordering::AcqRel);
      return Err(io::ErrorKind::OutOfMemory.into());
    };
    Ok(Self {
      allocation: AllocationLease::new(Arc::new(Allocation {
        pointer,
        capacity,
        budget,
        recyclable,
        charged: true,
        return_slot: None,
        receive_credit: None,
      })),
      offset: 0,
      capacity,
      length: 0,
    })
  }

  /// Write initialized data without leaving an uninitialized gap.
  ///
  /// # Errors
  /// Rejects gaps, overflow, and writes beyond capacity.
  pub fn write(&mut self, offset: usize, bytes: &[u8]) -> io::Result<()> {
    let end = offset.checked_add(bytes.len()).filter(|end| *end <= self.capacity);
    let Some(end) = end.filter(|_| offset <= self.length) else {
      return Err(io::ErrorKind::InvalidInput.into());
    };
    // SAFETY: Bounds were checked; exclusive ownership of this Buffer region prevents overlap with bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.pointer().add(offset), bytes.len()) };
    self.length = self.length.max(end);
    Ok(())
  }

  fn pointer(&self) -> *mut u8 {
    // SAFETY: Buffer geometry confines offset to its live allocation, including one-past-end empty regions.
    unsafe { self.allocation.pointer.as_ptr().add(self.offset) }
  }

  /// Divide exclusive storage into disjoint regions sharing one allocation charge.
  ///
  /// # Errors
  /// Returns the original buffer if the split exceeds capacity.
  pub fn split_at(self, at: usize) -> Result<(Self, Self), Self> {
    if at > self.capacity {
      return Err(self);
    }
    let tail = Self {
      allocation: self.allocation.clone(),
      offset: self.offset + at,
      capacity: self.capacity - at,
      length: self.length.saturating_sub(at),
    };
    let head = Self {
      capacity: at,
      length: self.length.min(at),
      ..self
    };
    Ok((head, tail))
  }

  /// Expose only initialized capacity, retaining the full backing allocation and budget charge.
  pub(crate) fn into_initialized(mut self) -> Self {
    self.capacity = self.length;
    self
  }

  /// Recover private append capacity only when no sibling can access the backing allocation.
  pub(crate) fn into_private(mut self) -> Self {
    if let Some(allocation) = Arc::get_mut(&mut self.allocation) {
      self.capacity = allocation.capacity - self.offset;
    }
    self
  }

  /// Transfer to immutable storage shared by sends and retained consumer views.
  pub fn freeze(self) -> FrozenBuffer {
    FrozenBuffer {
      allocation: self.allocation,
      offset: self.offset,
      capacity: self.capacity,
      length: self.length,
    }
  }
}

impl IoBuf for Buffer {
  fn as_init(&self) -> &[u8] {
    // SAFETY: length covers only initialized bytes in this live owned region.
    unsafe { std::slice::from_raw_parts(self.pointer(), self.length) }
  }
}

impl SetLen for Buffer {
  unsafe fn set_len(&mut self, length: usize) {
    assert!(length <= self.capacity);
    self.length = length;
  }
}

impl IoBufMut for Buffer {
  fn as_uninit(&mut self) -> &mut [MaybeUninit<u8>] {
    // SAFETY: This Buffer exclusively owns capacity bytes; MaybeUninit permits unread initialization state.
    unsafe { std::slice::from_raw_parts_mut(self.pointer().cast(), self.capacity) }
  }
}

/// Fully initialized, private TLS encoding storage. Only this wrapper can admit storage to
/// the TLS output pool; ordinary mutable recovery removes that admission before exposing it.
#[derive(Debug)]
pub(crate) struct TlsOutput(Buffer);

impl TlsOutput {
  pub(crate) fn new(capacity: usize, budget: Budget) -> io::Result<Self> {
    if capacity == 0 || capacity > isize::MAX as usize {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let buffer = if let Some(allocation) = Pool::take(capacity, &budget, Reuse::TlsOutput) {
      Buffer {
        allocation: AllocationLease::new(allocation),
        offset: 0,
        capacity,
        length: capacity,
      }
    } else {
      // Do not mark fresh storage recyclable until every byte has been initialized.
      let mut buffer = Buffer::allocate(capacity, budget, None)?;
      buffer.ensure_init();
      buffer.length = capacity;
      Arc::get_mut(&mut buffer.allocation).unwrap().recyclable = Some(Reuse::TlsOutput);
      buffer
    };
    Ok(Self(buffer))
  }

  pub(crate) fn as_mut(&mut self) -> &mut [u8] {
    // SAFETY: Construction initializes full capacity, and this wrapper never exposes
    // MaybeUninit or shared mutable access. Recycled allocations retain that invariant.
    unsafe { std::slice::from_raw_parts_mut(self.0.pointer(), self.0.capacity) }
  }

  pub(crate) fn freeze(mut self, length: usize) -> FrozenBuffer {
    assert!(length <= self.0.capacity);
    self.0.length = length;
    self.0.freeze()
  }
}

/// Immutable view retaining native storage independently of its connection or request.
#[derive(Clone, Debug)]
pub struct FrozenBuffer {
  allocation: AllocationLease,
  offset: usize,
  capacity: usize,
  length: usize,
}

impl FrozenBuffer {
  /// Join adjacent initialized views of the same allocation without copying or widening access.
  pub(crate) fn merge(&mut self, next: Self) -> Result<(), Self> {
    if !Arc::ptr_eq(&self.allocation, &next.allocation) || self.offset + self.length != next.offset {
      return Err(next);
    }
    self.length += next.length;
    self.capacity = self.length;
    Ok(())
  }

  /// Full backing allocation retained by this view, including bytes outside its slice.
  #[cfg(any(target_os = "linux", test))]
  pub(crate) fn retained_capacity(&self) -> usize {
    self.allocation.capacity
  }

  /// Shared read-ahead charge for provided receives, independent of storage lifetime.
  pub(crate) fn receive_credit(&self) -> Option<provided::ReceiveCredit> {
    self.allocation.receive_credit.clone()
  }

  /// Retain a bounded initialized subregion without copying.
  ///
  /// # Errors
  /// Rejects reversed or out-of-bounds ranges.
  pub fn slice(&self, range: Range<usize>) -> io::Result<Self> {
    if range.start > range.end || range.end > self.length {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(Self {
      allocation: self.allocation.clone(),
      offset: self.offset + range.start,
      capacity: range.len(),
      length: range.len(),
    })
  }

  /// Regain mutation only after every other view and send has released its lease.
  ///
  /// # Errors
  /// Returns the unchanged view if another owner exists.
  pub fn try_into_mut(mut self) -> Result<Buffer, Self> {
    if Arc::strong_count(&self.allocation) != 1 {
      return Err(self);
    }
    if self.allocation.recyclable == Some(Reuse::TlsOutput) {
      // IoBufMut permits callers to write uninitialized bytes after mutable recovery.
      Arc::get_mut(&mut self.allocation).unwrap().recyclable = Some(Reuse::Exact);
    }
    Ok(Buffer {
      allocation: self.allocation,
      offset: self.offset,
      capacity: self.capacity,
      length: self.length,
    })
  }
}

impl AsRef<[u8]> for FrozenBuffer {
  fn as_ref(&self) -> &[u8] {
    // SAFETY: FrozenBuffer retains the allocation and its initialized immutable subrange.
    unsafe { std::slice::from_raw_parts(self.allocation.pointer.as_ptr().add(self.offset), self.length) }
  }
}

impl IoBuf for FrozenBuffer {
  fn as_init(&self) -> &[u8] {
    self.as_ref()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn tls_output_reuse_prefers_the_most_recent_initialized_allocation() {
    let budget = Budget::new(1024);
    let older = TlsOutput::new(32, budget.clone()).unwrap();
    let mut recent = TlsOutput::new(32, budget.clone()).unwrap();
    let pointer = recent.as_mut().as_ptr();
    drop(older);
    drop(recent);
    let mut reused = TlsOutput::new(32, budget.clone()).unwrap();
    assert_eq!(reused.as_mut().as_ptr(), pointer);
    assert_eq!(budget.used(), 32);
    drop(reused);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn tls_output_reuses_initialized_capacity_and_exposes_only_written_bytes() {
    let budget = Budget::new(1024);
    let mut output = TlsOutput::new(32, budget.clone()).unwrap();
    assert_eq!(output.as_mut(), &[0; 32]);
    output.as_mut().fill(0xa5);
    let frozen = output.freeze(3);
    let pointer = frozen.as_ref().as_ptr();
    assert_eq!(frozen.as_ref(), &[0xa5; 3]);
    assert_eq!(budget.used(), 32);
    drop(frozen);
    assert_eq!(budget.used(), 0);
    let mut output = TlsOutput::new(32, budget.clone()).unwrap();
    assert_eq!(output.as_mut().as_ptr(), pointer);
    assert_eq!(output.as_mut(), &[0xa5; 32], "recycling preserves initialized bytes");
    drop(output);
    assert_eq!(budget.used(), 0);
    budget.close();
    assert_eq!(
      TlsOutput::new(32, budget).unwrap_err().kind(),
      io::ErrorKind::BrokenPipe
    );
  }

  #[test]
  fn tls_output_retained_views_prevent_reuse_and_keep_budget_charges() {
    let budget = Budget::new(64);
    let mut output = TlsOutput::new(32, budget.clone()).unwrap();
    output.as_mut().fill(0xa5);
    let frozen = output.freeze(32);
    let retained = FrozenBuffer::slice(&frozen, 0..3).unwrap();
    drop(frozen);
    let mut next = TlsOutput::new(32, budget.clone()).unwrap();
    assert_ne!(next.as_mut().as_ptr(), retained.as_ref().as_ptr());
    assert_eq!(retained.as_ref(), &[0xa5; 3]);
    assert_eq!(budget.used(), 64);
    assert_eq!(
      TlsOutput::new(1, budget.clone()).unwrap_err().kind(),
      io::ErrorKind::OutOfMemory
    );
    drop(next);
    drop(retained);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn tls_output_mutable_recovery_invalidates_initialized_pool_membership() {
    let budget = Budget::new(1024);
    let output = TlsOutput::new(32, budget.clone()).unwrap();
    let mut recovered = output.freeze(3).try_into_mut().unwrap();
    recovered.as_uninit().fill(MaybeUninit::uninit());
    drop(recovered);
    let mut next = TlsOutput::new(32, budget.clone()).unwrap();
    assert_eq!(
      next.as_mut(),
      &[0; 32],
      "uninitialized storage cannot enter the TLS output pool"
    );
    drop(next);
    let mut foreign = Buffer::new(32, budget).unwrap();
    assert_eq!(foreign.ensure_init(), &[0; 32]);
  }

  #[test]
  fn racing_owner_close_preserves_live_charges_and_rejects_new_allocations() {
    for _ in 0..64 {
      let budget = Budget::new(1024);
      let retained = Buffer::new(32, budget.clone()).unwrap();
      let start = std::sync::Barrier::new(5);
      std::thread::scope(|scope| {
        for _ in 0..4 {
          let budget = &budget;
          let start = &start;
          scope.spawn(move || {
            start.wait();
            for _ in 0..32 {
              let closed = budget.is_closed();
              let allocation = Buffer::new(32, budget.clone());
              if closed {
                assert_eq!(allocation.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
              }
            }
          });
        }
        start.wait();
        budget.close();
      });
      assert_eq!(budget.used(), 32);
      drop(retained);
      assert_eq!(budget.used(), 0);
      assert_eq!(
        Buffer::new(1, budget.clone()).unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
      );
      assert_eq!(budget.used(), 0);
    }
  }

  #[test]
  fn response_batch_slices_merge_and_outlive_the_scope() {
    let budget = Budget::new(RESPONSE_SLAB);
    let scope = ResponseBatch::enter();
    let mut first = Buffer::response(3, &budget).unwrap();
    let mut second = Buffer::response(3, &budget).unwrap();
    first.write(0, b"abc").unwrap();
    second.write(0, b"def").unwrap();
    let mut first = first.freeze();
    first.merge(second.freeze()).unwrap();
    assert_eq!(first.as_ref(), b"abcdef");
    assert_eq!(budget.used(), RESPONSE_SLAB);
    drop(scope);
    assert_eq!(first.as_ref(), b"abcdef");
    std::thread::spawn(move || drop(first)).join().unwrap();
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn response_batch_is_nested_bounded_and_owner_specific() {
    let budget = Budget::new(RESPONSE_SLAB);
    let other = Budget::new(RESPONSE_SLAB);
    let outer = ResponseBatch::enter();
    let inner = ResponseBatch::enter();
    drop(Buffer::response(32, &budget).unwrap());
    drop(inner);
    assert_eq!(budget.used(), RESPONSE_SLAB);
    let held = Buffer::response(32, &other).unwrap();
    assert_eq!(budget.used(), 0, "switching owners drops the old spare tail");
    other.close();
    assert!(Buffer::response(32, &other).is_err());
    drop(held);
    drop(outer);
    assert_eq!(other.used(), 0);
    let exact = Buffer::response(32, &budget).unwrap();
    assert_eq!(budget.used(), 32, "outside callbacks no spare slab is held");
    drop(exact);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn response_batch_falls_back_to_exact_capacity_and_preserves_live_slices() {
    let budget = Budget::new(8);
    let scope = ResponseBatch::enter();
    let mut exact = Buffer::response(8, &budget).unwrap();
    exact.write(0, b"retained").unwrap();
    assert!(Buffer::response(1, &budget).is_err());
    assert!(Buffer::response(0, &budget).is_err());
    drop(scope);
    assert_eq!(exact.as_init(), b"retained");
    drop(exact);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn merged_response_never_exposes_spare_or_unrelated_bytes() {
    let budget = Budget::new(RESPONSE_SLAB * 2);
    let _scope = ResponseBatch::enter();
    let mut first = Buffer::response(8, &budget).unwrap();
    first.write(0, b"abc").unwrap();
    let mut second = Buffer::response(3, &budget).unwrap();
    second.write(0, b"def").unwrap();
    let mut first = first.freeze();
    let second = first.merge(second.freeze()).unwrap_err();
    assert_eq!(first.as_ref(), b"abc");
    assert_eq!(second.as_ref(), b"def");
    let mut unrelated = Buffer::new(3, budget).unwrap();
    unrelated.write(0, b"ghi").unwrap();
    assert!(first.merge(unrelated.freeze()).is_err());
  }

  #[test]
  fn private_receive_recovers_spare_capacity_without_exposing_uninitialized_bytes() {
    let budget = Budget::new(32);
    let mut buffer = Buffer::new(32, budget.clone()).unwrap();
    buffer.write(0, b"abc").unwrap();
    let pointer = buffer.pointer();
    let mut buffer = buffer.into_initialized();
    assert_eq!(buffer.buf_capacity(), 3);
    assert_eq!(buffer.as_init(), b"abc");
    let mut buffer = buffer.into_private();
    assert_eq!(buffer.pointer(), pointer);
    assert_eq!(buffer.buf_capacity(), 32);
    assert_eq!(buffer.as_init(), b"abc");
    buffer.write(3, b"def").unwrap();
    assert_eq!(buffer.as_init(), b"abcdef");
    assert_eq!(budget.used(), 32);
    drop(buffer);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn private_capacity_respects_retained_siblings_and_allocation_offset() {
    let budget = Budget::new(16);
    let mut buffer = Buffer::new(16, budget.clone()).unwrap();
    buffer.write(0, b"abcdefghij").unwrap();
    let (head, tail) = buffer.split_at(8).unwrap();
    let tail = tail.freeze();
    let mut head = head.into_initialized().into_private();
    assert_eq!(head.buf_capacity(), 8);
    assert!(head.write(8, b"!").is_err());
    assert_eq!(tail.as_ref(), b"ij");
    drop(head);
    let mut tail = tail.try_into_mut().unwrap().into_initialized();
    assert_eq!(tail.buf_capacity(), 2);
    let mut tail = tail.into_private();
    assert_eq!(tail.buf_capacity(), 8);
    assert_eq!(tail.as_init(), b"ij");
    tail.write(2, b"klmnop").unwrap();
    assert_eq!(tail.as_init(), b"ijklmnop");
    assert!(tail.write(8, b"!").is_err());
    assert_eq!(budget.used(), 16);
    drop(tail);
    assert_eq!(budget.used(), 0);
  }
}
