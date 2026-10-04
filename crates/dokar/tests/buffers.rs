use dokar::buffer::{Budget, Buffer};

#[test]
fn retained_slice_keeps_storage_and_budget_alive() {
  let budget = Budget::new(16);
  let mut buffer = Buffer::new(16, budget.clone()).unwrap();
  buffer.write(0, b"hello").unwrap();
  let frozen = buffer.freeze();
  let slice = frozen.slice(1..4).unwrap();
  let address = slice.as_ref().as_ptr();
  drop(frozen);
  assert_eq!(slice.as_ref(), b"ell");
  assert_eq!(slice.as_ref().as_ptr(), address);
  assert_eq!(budget.used(), 16);
  assert!(Buffer::new(1, budget.clone()).is_err());
  drop(slice);
  assert_eq!(budget.used(), 0);
  assert!(Buffer::new(16, budget).is_ok());
}

#[test]
fn rejects_uninitialized_and_overflowing_ranges() {
  let mut buffer = Buffer::new(8, Budget::new(8)).unwrap();
  assert!(buffer.write(1, b"x").is_err());
  assert!(buffer.write(usize::MAX, b"x").is_err());
  buffer.write(0, b"abc").unwrap();
  let frozen = buffer.freeze();
  assert!(frozen.slice(0..4).is_err());
  assert!(frozen.slice(usize::MAX..usize::MAX).is_err());
  assert_eq!(frozen.as_ref(), b"abc");
}

#[test]
fn mutation_requires_unique_ownership() {
  let mut buffer = Buffer::new(8, Budget::new(8)).unwrap();
  buffer.write(0, b"abc").unwrap();
  let frozen = buffer.freeze();
  let retained = frozen.clone();
  let frozen = frozen.try_into_mut().unwrap_err();
  drop(retained);
  let mut buffer = frozen.try_into_mut().unwrap();
  buffer.write(1, b"z").unwrap();
  assert_eq!(buffer.freeze().as_ref(), b"azc");
}

#[test]
fn retained_view_can_be_released_on_another_thread() {
  let budget = Budget::new(32);
  let buffer = Buffer::new(32, budget.clone()).unwrap().freeze();
  std::thread::spawn(move || drop(buffer)).join().unwrap();
  assert_eq!(budget.used(), 0);
}

#[test]
fn disjoint_regions_can_retain_plaintext_while_reusing_the_tail() {
  let budget = Budget::new(16);
  let mut buffer = Buffer::new(16, budget.clone()).unwrap();
  buffer.write(0, b"plaintexttail").unwrap();
  let (head, mut tail) = buffer.split_at(9).unwrap();
  let head = head.freeze();
  tail.write(0, b"next").unwrap();
  assert_eq!(head.as_ref(), b"plaintext");
  assert_eq!(tail.freeze().as_ref(), b"next");
  assert_eq!(budget.used(), 16);
  drop(head);
  assert_eq!(budget.used(), 0);
}

#[test]
fn closed_owner_prevents_allocations_through_retained_clones() {
  let budget = Budget::new(128);
  let retained = budget.clone();
  let mut buffer = Buffer::new(64, retained.clone()).unwrap();
  buffer.write(0, b"retained").unwrap();
  budget.close();
  assert!(Buffer::new(1, retained).is_err());
  assert_eq!(buffer.freeze().as_ref(), b"retained");
  assert_eq!(budget.used(), 0);
}

#[test]
fn initialization_clears_poisoned_spare_capacity() {
  use compio_buf::{IoBuf, IoBufMut};
  use std::mem::MaybeUninit;

  let budget = Budget::new(32);
  let mut buffer = Buffer::new(32, budget.clone()).unwrap();
  buffer.as_uninit().fill(MaybeUninit::new(0xa5));
  assert_eq!(buffer.buf_len(), 0);
  assert_eq!(buffer.ensure_init(), &[0; 32]);

  buffer.write(0, b"hello").unwrap();
  buffer.as_uninit()[5..].fill(MaybeUninit::new(0xa5));
  let initialized = buffer.ensure_init();
  assert_eq!(&initialized[..5], b"hello");
  assert_eq!(&initialized[5..], &[0; 27]);
  assert_eq!(budget.used(), 32);
  drop(buffer);
  assert_eq!(budget.used(), 0);
}

#[test]
fn allocations_reject_zero_and_oversized_capacities() {
  let budget = Budget::new(64);
  assert_eq!(
    Buffer::new(0, budget.clone()).unwrap_err().kind(),
    std::io::ErrorKind::InvalidInput
  );
  assert_eq!(
    Buffer::new(usize::MAX, budget.clone()).unwrap_err().kind(),
    std::io::ErrorKind::InvalidInput
  );
  assert_eq!(
    Buffer::receive(0, budget.clone()).unwrap_err().kind(),
    std::io::ErrorKind::InvalidInput
  );
  assert_eq!(budget.used(), 0);
}

#[test]
fn split_beyond_capacity_returns_the_original_buffer() {
  let budget = Budget::new(8);
  let mut buffer = Buffer::new(8, budget.clone()).unwrap();
  buffer.write(0, b"abcd").unwrap();
  let buffer = buffer.split_at(9).unwrap_err();
  let (head, tail) = buffer.split_at(8).unwrap();
  assert_eq!(head.freeze().as_ref(), b"abcd");
  assert!(tail.freeze().as_ref().is_empty());
  assert_eq!(budget.used(), 0);
}

#[test]
fn pooled_storage_of_a_closed_owner_is_discarded_instead_of_reused() {
  use dokar::buffer::pool_retained;
  let budget = Budget::new(64 * 1024);
  let before = pool_retained();
  drop(Buffer::receive(4096, budget.clone()).unwrap());
  assert_eq!(pool_retained(), before + 4096, "released receive storage is pooled");
  assert_eq!(budget.used(), 0, "pooled storage is not charged");
  budget.close();
  assert!(Buffer::receive(4096, budget.clone()).is_err());
  assert_eq!(pool_retained(), before, "a closed owner's pooled storage is freed");
  assert_eq!(budget.used(), 0);
}
