/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Owner-shared provided storage with connection-scoped delivery credit. Only the owner publishes or retires kernel buffers.

#[cfg(any(target_os = "linux", test))]
use super::{Allocation, Buffer};
use super::{Budget, mi_free};
use std::ffi::c_void;
#[cfg(any(target_os = "linux", test))]
use std::io;
#[cfg(any(target_os = "linux", test))]
use std::marker::PhantomData;
#[cfg(any(target_os = "linux", test))]
use std::ptr;
use std::ptr::NonNull;
#[cfg(any(target_os = "linux", test))]
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::task::Waker;

pub(crate) const SLOT_BYTES: usize = 16 * 1024;

/// Exactly one allocation can be returned to a slot. A release never touches ring metadata.
#[derive(Debug)]
pub(super) struct ReturnSlot {
  pointer: AtomicPtr<u8>,
  budget: Budget,
  wake: Option<Waker>,
}

impl ReturnSlot {
  pub(super) fn release(&self, pointer: NonNull<u8>) {
    // Only the final Allocation drop publishes, and the owner consumes before creating its successor.
    let previous = self.pointer.swap(pointer.as_ptr(), Ordering::AcqRel);
    assert!(previous.is_null(), "receive allocation returned twice");
    if let Some(wake) = &self.wake {
      wake.wake_by_ref();
    }
  }

  #[cfg(any(target_os = "linux", test))]
  fn take(self: &Arc<Self>) -> Option<Buffer> {
    let pointer = NonNull::new(self.pointer.swap(ptr::null_mut(), Ordering::AcqRel))?;
    Some(Buffer {
      allocation: super::AllocationLease::new(Arc::new(Allocation {
        pointer,
        capacity: SLOT_BYTES,
        budget: self.budget.clone(),
        recyclable: None,
        charged: true,
        return_slot: Some(self.clone()),
        receive_credit: None,
      })),
      offset: 0,
      capacity: SLOT_BYTES,
      length: 0,
    })
  }
}

impl Drop for ReturnSlot {
  fn drop(&mut self) {
    let pointer = *self.pointer.get_mut();
    if !pointer.is_null() {
      // SAFETY: The last ReturnSlot owns this mi_malloc allocation, returned to the paired allocator.
      unsafe { mi_free(pointer.cast::<c_void>()) };
      self.budget.0.used.fetch_sub(SLOT_BYTES, Ordering::AcqRel);
    }
  }
}

#[derive(Debug)]
struct Window {
  charged: AtomicUsize,
  #[cfg(any(target_os = "linux", test))]
  limit: usize,
  wake: Option<Waker>,
}

#[derive(Debug)]
struct Credit {
  window: Arc<Window>,
  acknowledged: AtomicBool,
}

impl Credit {
  fn ack(&self) {
    if !self.acknowledged.swap(true, Ordering::AcqRel) {
      self.window.charged.fetch_sub(SLOT_BYTES, Ordering::AcqRel);
      if let Some(wake) = &self.window.wake {
        wake.wake_by_ref();
      }
    }
  }
}

impl Drop for Credit {
  fn drop(&mut self) {
    self.ack();
  }
}

/// Clones share one receive's capacity charge; acknowledgement never releases storage.
#[derive(Clone, Debug)]
pub struct ReceiveCredit(Arc<Credit>);

impl ReceiveCredit {
  /// Return read-ahead credit without releasing retained bytes. Repeated calls are harmless.
  pub fn ack(&self) {
    self.0.ack();
  }
}

#[derive(Debug)]
#[cfg(any(target_os = "linux", test))]
pub(crate) struct Descriptor {
  pub(crate) id: u16,
  pub(crate) pointer: *mut u8,
  pub(crate) capacity: usize,
}

/// Delivery credit belongs to a connection; shared published storage does not.
#[cfg(any(target_os = "linux", test))]
pub(crate) struct ReceiveWindow(Arc<Window>);

#[cfg(any(target_os = "linux", test))]
impl ReceiveWindow {
  pub(crate) fn new(limit: usize, wake: Waker) -> io::Result<Self> {
    if limit < SLOT_BYTES {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(Self(Arc::new(Window {
      charged: AtomicUsize::new(0),
      limit,
      wake: Some(wake),
    })))
  }

  pub(crate) fn available(&self) -> bool {
    self.0.charged.load(Ordering::Acquire) <= self.0.limit - SLOT_BYTES
  }

  pub(crate) fn attach(&self, buffer: &mut Buffer) -> Option<ReceiveCredit> {
    if !self.available() {
      return None;
    }
    self.0.charged.fetch_add(SLOT_BYTES, Ordering::AcqRel);
    let credit = ReceiveCredit(Arc::new(Credit {
      window: self.0.clone(),
      acknowledged: AtomicBool::new(false),
    }));
    let allocation = Arc::get_mut(&mut buffer.allocation).expect("undelivered receive is exclusive");
    debug_assert!(allocation.receive_credit.is_none());
    allocation.receive_credit = Some(credit.clone());
    Some(credit)
  }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) const SHARED_SLOTS: usize = 32;

#[cfg(any(target_os = "linux", test))]
struct SharedSlot {
  returned: Arc<ReturnSlot>,
  available: Option<Buffer>,
  published: Option<Buffer>,
}

/// One bounded pool per owner budget. Start small; grow only after kernel exhaustion.
#[cfg(any(target_os = "linux", test))]
pub(crate) struct SharedReceivePool {
  slots: Vec<SharedSlot>,
  budget: Budget,
  wake: Waker,
  _owner: PhantomData<Rc<()>>,
}

#[cfg(any(target_os = "linux", test))]
impl SharedReceivePool {
  pub(crate) fn new(budget: Budget, wake: Waker) -> io::Result<Self> {
    let mut pool = Self {
      slots: Vec::new(),
      budget,
      wake,
      _owner: PhantomData,
    };
    pool.grow()?;
    Ok(pool)
  }

  pub(crate) fn len(&self) -> usize {
    self.slots.len()
  }

  pub(crate) fn grow(&mut self) -> io::Result<()> {
    let target = (self.slots.len() * 2).clamp(2, SHARED_SLOTS);
    while self.slots.len() < target {
      let (buffer, returned) = self.allocate()?;
      self.slots.push(SharedSlot {
        returned,
        available: Some(buffer),
        published: None,
      });
    }
    Ok(())
  }

  fn allocate(&self) -> io::Result<(Buffer, Arc<ReturnSlot>)> {
    let mut buffer = Buffer::new(SLOT_BYTES, self.budget.clone())?;
    let returned = Arc::new(ReturnSlot {
      pointer: AtomicPtr::new(ptr::null_mut()),
      budget: self.budget.clone(),
      wake: Some(self.wake.clone()),
    });
    let allocation = Arc::get_mut(&mut buffer.allocation).expect("new allocation is exclusive");
    allocation.recyclable = None;
    allocation.return_slot = Some(returned.clone());
    Ok((buffer, returned))
  }

  pub(crate) fn publish(&mut self, id: u16) -> io::Result<Descriptor> {
    let index = usize::from(id);
    let slot = self.slots.get_mut(index).ok_or(io::ErrorKind::InvalidData)?;
    if slot.published.is_some() {
      return Err(io::ErrorKind::WouldBlock.into());
    }
    if slot.available.is_none() {
      slot.available = slot.returned.take();
    }
    if slot.available.is_none() {
      // Retained old views return to their detached target, never the replacement's slot.
      let (buffer, returned) = self.allocate()?;
      self.slots[index].available = Some(buffer);
      self.slots[index].returned = returned;
    }
    let slot = &mut self.slots[index];
    let buffer = slot.available.take().unwrap();
    let descriptor = Descriptor {
      id,
      pointer: buffer.pointer(),
      capacity: SLOT_BYTES,
    };
    slot.published = Some(buffer);
    Ok(descriptor)
  }

  /// # Safety
  /// A buffer-selection CQE for this registration must prove exactly `length` bytes were
  /// initialized and the kernel no longer accesses the selected descriptor.
  pub(crate) unsafe fn complete(&mut self, id: u16, length: usize) -> io::Result<Buffer> {
    if length > SLOT_BYTES {
      return Err(io::ErrorKind::InvalidData.into());
    }
    let mut buffer = self
      .slots
      .get_mut(usize::from(id))
      .ok_or(io::ErrorKind::InvalidData)?
      .published
      .take()
      .ok_or(io::ErrorKind::InvalidData)?;
    buffer.length = length;
    Ok(buffer)
  }

  /// # Safety
  /// Every target/cancel CQE must have retired and the buffer group must be unregistered.
  pub(crate) unsafe fn retire_published(&mut self) {
    for slot in &mut self.slots {
      if let Some(buffer) = slot.published.take() {
        slot.available = Some(buffer);
      }
    }
  }
}

#[cfg(any(target_os = "linux", test))]
impl Drop for SharedReceivePool {
  fn drop(&mut self) {
    for slot in &mut self.slots {
      if let Some(buffer) = slot.published.take() {
        #[expect(
          clippy::mem_forget,
          reason = "published buffers cannot be freed before kernel retirement"
        )]
        std::mem::forget(buffer);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use compio_buf::IoBuf;

  #[derive(Default)]
  struct Notify(AtomicUsize);
  impl std::task::Wake for Notify {
    fn wake(self: Arc<Self>) {
      self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
      self.0.fetch_add(1, Ordering::Relaxed);
    }
  }
  fn wake() -> Waker {
    Waker::from(Arc::new(Notify::default()))
  }

  #[test]
  fn pooled_receive_storage_releases_delivery_credit_on_final_view_drop() {
    let budget = Budget::new(8 * SLOT_BYTES);
    let window = ReceiveWindow::new(SLOT_BYTES, wake()).unwrap();
    let mut buffer = Buffer::receive(SLOT_BYTES, budget.clone()).unwrap();
    let pointer = buffer.buf_ptr();
    drop(window.attach(&mut buffer).unwrap());
    let frozen = buffer.freeze();
    let retained = frozen.clone();
    assert!(!window.available());
    drop(frozen);
    assert!(!window.available(), "a live view still owns its delivery credit");
    drop(retained);
    assert!(window.available(), "idle storage must not pin delivery credit");
    assert_eq!(budget.used(), 0);
    let mut reused = Buffer::receive(SLOT_BYTES, budget.clone()).unwrap();
    assert_eq!(reused.buf_ptr(), pointer);
    assert!(reused.receive_credit().is_none());
    drop(window.attach(&mut reused).unwrap());
    assert!(!window.available());
    drop(reused);
    assert!(window.available());
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn pooled_receive_storage_preserves_external_delivery_credit() {
    let budget = Budget::new(8 * SLOT_BYTES);
    let first = ReceiveWindow::new(SLOT_BYTES, wake()).unwrap();
    let second = ReceiveWindow::new(SLOT_BYTES, wake()).unwrap();
    let mut buffer = Buffer::receive(SLOT_BYTES, budget.clone()).unwrap();
    let pointer = buffer.buf_ptr();
    let credit = first.attach(&mut buffer).unwrap();
    drop(buffer);
    assert_eq!(budget.used(), 0);
    assert!(
      !first.available(),
      "external credit retains its independent reservation"
    );
    let mut reused = Buffer::receive(SLOT_BYTES, budget.clone()).unwrap();
    assert_eq!(reused.buf_ptr(), pointer);
    assert!(reused.receive_credit().is_none());
    assert!(first.attach(&mut reused).is_none());
    drop(second.attach(&mut reused).unwrap());
    assert!(!second.available());
    credit.ack();
    credit.ack();
    assert!(first.available());
    assert!(
      !second.available(),
      "old acknowledgement cannot release the new delivery"
    );
    drop(reused);
    assert!(second.available());
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn shared_storage_preserves_independent_delivery_credit_and_retained_views() {
    let budget = Budget::new(4 * SLOT_BYTES);
    let mut pool = SharedReceivePool::new(budget.clone(), wake()).unwrap();
    let first = ReceiveWindow::new(SLOT_BYTES, wake()).unwrap();
    let second = ReceiveWindow::new(SLOT_BYTES, wake()).unwrap();
    let descriptor = pool.publish(0).unwrap();
    assert_eq!(descriptor.id, 0);
    assert_eq!(descriptor.capacity, SLOT_BYTES);
    // SAFETY: The published descriptor owns at least one writable byte and no kernel operation is active.
    unsafe {
      descriptor.pointer.write(b'a');
    }
    // SAFETY: This test initialized one byte and simulates completion without an active kernel operation.
    let mut a = unsafe { pool.complete(0, 1) }.unwrap();
    let credit = first.attach(&mut a).unwrap();
    assert!(a.receive_credit().is_some());
    assert!(!first.available());
    let replacement = pool.publish(0).unwrap();
    assert_ne!(replacement.pointer, descriptor.pointer);
    // SAFETY: The replacement descriptor owns one writable byte and no kernel operation is active.
    unsafe {
      replacement.pointer.write(b'b');
    }
    // SAFETY: The replacement byte was initialized above; no kernel operation remains active.
    let mut b = unsafe { pool.complete(0, 1) }.unwrap();
    assert!(first.attach(&mut b).is_none());
    assert!(
      second.attach(&mut b).is_some(),
      "another connection keeps its own credit"
    );
    credit.ack();
    credit.ack();
    assert!(first.available());
    assert_eq!(a.as_init(), b"a");
    assert_eq!(b.as_init(), b"b");
    assert_eq!(budget.used(), 3 * SLOT_BYTES);
    drop(a);
    drop(b);
    // SAFETY: This test never submitted published buffers to the kernel; they are safe to retire.
    unsafe {
      pool.retire_published();
    }
    drop(pool);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn final_release_and_acknowledgement_notify_different_owners() {
    let storage = Arc::new(Notify::default());
    let delivery = Arc::new(Notify::default());
    let budget = Budget::new(2 * SLOT_BYTES);
    let mut pool = SharedReceivePool::new(budget.clone(), Waker::from(storage.clone())).unwrap();
    let window = ReceiveWindow::new(SLOT_BYTES, Waker::from(delivery.clone())).unwrap();
    let old = pool.publish(0).unwrap();
    // SAFETY: A zero-byte completion exposes no uninitialized bytes; this test submits no kernel I/O.
    let mut buffer = unsafe { pool.complete(0, 0) }.unwrap();
    let credit = window.attach(&mut buffer).unwrap();
    credit.ack();
    assert_eq!(delivery.0.load(Ordering::Relaxed), 1);
    assert_eq!(storage.0.load(Ordering::Relaxed), 0);
    assert_eq!(pool.publish(0).unwrap_err().kind(), io::ErrorKind::OutOfMemory);
    std::thread::spawn(move || drop(buffer)).join().unwrap();
    assert_eq!(storage.0.load(Ordering::Relaxed), 1);
    assert_eq!(pool.publish(0).unwrap().pointer, old.pointer);
    // SAFETY: This test never submitted these buffers to the kernel.
    unsafe {
      pool.retire_published();
    }
    drop(pool);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn shared_pool_growth_and_completion_validation_are_bounded() {
    let budget = Budget::new(SHARED_SLOTS * SLOT_BYTES);
    let mut pool = SharedReceivePool::new(budget.clone(), wake()).unwrap();
    assert_eq!(pool.len(), 2);
    for _ in 0..8 {
      pool.grow().unwrap();
    }
    assert_eq!(pool.len(), SHARED_SLOTS);
    assert_eq!(budget.used(), SHARED_SLOTS * SLOT_BYTES);
    pool.publish(0).unwrap();
    assert!(pool.publish(0).is_err());
    // SAFETY: This invalid slot is rejected before access; the test submits no kernel I/O.
    assert!(unsafe { pool.complete(SHARED_SLOTS as u16, 0) }.is_err());
    // SAFETY: The oversized length is rejected before access; the test submits no kernel I/O.
    assert!(unsafe { pool.complete(0, SLOT_BYTES + 1) }.is_err());
    // SAFETY: Zero bytes require no initialization; this test submits no kernel I/O.
    let buffer = unsafe { pool.complete(0, 0) }.unwrap();
    // SAFETY: The already-consumed slot is rejected before access; no kernel I/O exists.
    assert!(unsafe { pool.complete(0, 0) }.is_err());
    drop(pool);
    budget.close();
    assert_eq!(budget.used(), SLOT_BYTES);
    drop(buffer);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn insufficient_initial_budget_rolls_back_storage() {
    let budget = Budget::new(SLOT_BYTES);
    assert!(SharedReceivePool::new(budget.clone(), wake()).is_err());
    assert_eq!(budget.used(), 0);
  }
}
