use bemo::buffer::{Budget, Buffer, FrozenBuffer, pool_retained, pool_trim};
use compio_buf::{IoBuf, IoBufMut};

fn drain() {
  while pool_retained() > 0 {
    pool_trim();
  }
}

fn address(buffer: &Buffer) -> *const u8 {
  buffer.as_init().as_ptr()
}

#[test]
fn receive_storage_returns_to_the_pool_and_is_reused() {
  drain();
  let budget = Budget::new(1024 * 1024);
  let first = Buffer::receive(4096, budget.clone()).unwrap();
  let pointer = address(&first);
  drop(first);
  assert_eq!(pool_retained(), 4096);
  assert_eq!(budget.used(), 0);
  let second = Buffer::receive(4096, budget.clone()).unwrap();
  assert_eq!(address(&second), pointer);
  assert_eq!(pool_retained(), 0);
  assert_eq!(budget.used(), 4096);
  drop(second);
  drain();
  assert_eq!(budget.used(), 0);
}

#[test]
fn ordinary_storage_is_admitted_with_exact_geometry() {
  drain();
  let budget = Budget::new(1024 * 1024);
  let buffer = Buffer::new(4096, budget.clone()).unwrap();
  let pointer = address(&buffer);
  drop(buffer);
  assert_eq!(pool_retained(), 4096);
  assert_eq!(budget.used(), 0);
  let larger = Buffer::new(4097, budget.clone()).unwrap();
  assert_ne!(address(&larger), pointer);
  assert_eq!(pool_retained(), 4096);
  let mut exact = Buffer::new(4096, budget).unwrap();
  assert_eq!(address(&exact), pointer);
  assert_eq!(exact.buf_capacity(), 4096);
  assert_eq!(exact.buf_len(), 0);
  drop((exact, larger));
  drain();
}

#[test]
fn exact_reuse_prefers_the_most_recent_matching_allocation() {
  drain();
  let budget = Budget::new(8192);
  let older = Buffer::new(1024, budget.clone()).unwrap();
  let recent = Buffer::new(1024, budget.clone()).unwrap();
  let recent_pointer = address(&recent);
  drop(older);
  drop(recent);
  let reused = Buffer::new(1024, budget.clone()).unwrap();
  assert_eq!(address(&reused), recent_pointer);
  assert_eq!(budget.used(), 1024);
  assert_eq!(pool_retained(), 1024);
  drop(reused);
  drain();
}

#[test]
fn receive_reuse_keeps_smallest_fit_and_owner_isolation() {
  drain();
  let budget = Budget::new(65536);
  let other = Budget::new(65536);
  let exact = Buffer::receive(1024, budget.clone()).unwrap();
  let exact_pointer = address(&exact);
  let larger = Buffer::receive(2048, budget.clone()).unwrap();
  let foreign = Buffer::receive(512, other.clone()).unwrap();
  drop(exact);
  drop(larger);
  drop(foreign);
  let reused = Buffer::receive(512, budget.clone()).unwrap();
  assert_eq!(address(&reused), exact_pointer);
  assert_eq!(budget.used(), 1024);
  assert_eq!(other.used(), 0);
  assert_eq!(pool_retained(), 2560);
  drop(reused);
  drain();
}

#[test]
fn retained_leases_prevent_admission() {
  drain();
  let budget = Budget::new(1024 * 1024);
  let mut buffer = Buffer::receive(4096, budget).unwrap();
  buffer.write(0, b"hello").unwrap();
  let frozen = buffer.freeze();
  let slice = FrozenBuffer::slice(&frozen, 1..4).unwrap();
  drop(frozen);
  assert_eq!(pool_retained(), 0);
  assert_eq!(slice.as_ref(), b"ell");
  drop(slice);
  assert_eq!(pool_retained(), 4096);
  drain();
}

#[test]
fn a_different_owner_cannot_take_pooled_storage() {
  drain();
  let budget = Budget::new(1024 * 1024);
  drop(Buffer::receive(4096, budget).unwrap());
  assert_eq!(pool_retained(), 4096);
  let other = Budget::new(1024 * 1024);
  let buffer = Buffer::receive(4096, other.clone()).unwrap();
  assert_eq!(pool_retained(), 4096);
  assert_eq!(other.used(), 4096);
  drop(buffer);
  drain();
}

#[test]
fn entry_count_bounds_the_pool() {
  drain();
  let budget = Budget::new(1024 * 1024);
  // Strictly increasing sizes never fit an existing entry, so every drop attempts a new admission.
  let sizes: Vec<usize> = (0..20).map(|index| 1024 + index * 64).collect();
  for size in &sizes {
    drop(Buffer::receive(*size, budget.clone()).unwrap());
  }
  let expected: usize = sizes[..16].iter().sum();
  assert_eq!(pool_retained(), expected);
  assert_eq!(budget.used(), 0);
  drain();
  assert_eq!(pool_retained(), 0);
}

#[test]
fn retained_bytes_are_bounded_by_the_owner_budget() {
  drain();
  // A 64 KiB owner permits 8 KiB of pooled capacity, so the second, larger allocation is refused.
  let budget = Budget::new(64 * 1024);
  drop(Buffer::receive(4096, budget.clone()).unwrap());
  drop(Buffer::receive(8192, budget.clone()).unwrap());
  assert_eq!(pool_retained(), 4096);
  assert_eq!(budget.used(), 0);
  drain();
  assert_eq!(pool_retained(), 0);
}

#[test]
fn oversized_allocations_are_never_pooled() {
  drain();
  let budget = Budget::new(8 * 1024 * 1024);
  drop(Buffer::receive(512 * 1024, budget.clone()).unwrap());
  assert_eq!(pool_retained(), 0);
  assert_eq!(budget.used(), 0);
}

#[test]
fn trimming_prefers_storage_whose_owner_was_released() {
  drain();
  let live = Budget::new(1024 * 1024);
  let closed = Budget::new(1024 * 1024);
  drop(Buffer::receive(1024, live.clone()).unwrap());
  drop(Buffer::receive(2048, closed.clone()).unwrap());
  closed.close();
  assert_eq!(pool_retained(), 3072);
  assert_eq!(live.used(), 0);
  pool_trim();
  assert_eq!(pool_retained(), 1024);
  drain();
  assert_eq!(pool_retained(), 0);
}

#[test]
fn a_closed_owner_cannot_admit_new_storage() {
  drain();
  let budget = Budget::new(1024 * 1024);
  let buffer = Buffer::receive(4096, budget.clone()).unwrap();
  budget.close();
  drop(buffer);
  assert_eq!(pool_retained(), 0);
  assert_eq!(budget.used(), 0);
}

#[test]
fn cross_thread_release_pools_on_the_releasing_thread() {
  drain();
  let budget = Budget::new(1024 * 1024);
  let buffer = Buffer::receive(4096, budget.clone()).unwrap();
  let observed = std::thread::spawn(move || {
    drop(buffer);
    let retained = pool_retained();
    let _reused = Buffer::receive(4096, budget.clone()).unwrap();
    (retained, pool_retained(), budget.used())
  })
  .join()
  .unwrap();
  assert_eq!(observed, (4096, 0, 4096));
  assert_eq!(pool_retained(), 0);
}

#[test]
fn ordinary_storage_changes_owner_only_after_last_lease_and_reserves_again() {
  drain();
  let old = Budget::new(8192);
  let mut storage = Buffer::new(4096, old.clone()).unwrap();
  storage.write(0, b"secret").unwrap();
  let pointer = address(&storage);
  let frozen = storage.freeze();
  let lease = FrozenBuffer::slice(&frozen, 1..4).unwrap();
  drop(frozen);
  assert_eq!(pool_retained(), 0);
  assert_eq!(old.used(), 4096);
  drop(lease);
  assert_eq!(pool_retained(), 4096);
  assert_eq!(old.used(), 0);
  old.close();
  let new = Budget::new(8192);
  let mut reused = Buffer::new(4096, new.clone()).unwrap();
  assert_eq!(address(&reused), pointer);
  assert_eq!(reused.as_init(), b"");
  assert_eq!(new.used(), 4096);
  assert_eq!(old.used(), 0);
  assert!(reused.ensure_init().iter().all(|&byte| byte == 0));
  drop(reused);
  assert_eq!(new.used(), 0);
  drain();
}

#[test]
fn ordinary_reuse_obeys_new_owner_limits_and_closure() {
  drain();
  drop(Buffer::new(4096, Budget::new(8192)).unwrap());
  let small = Budget::new(4095);
  assert!(Buffer::new(4096, small.clone()).is_err());
  assert_eq!(small.used(), 0);
  let closed = Budget::new(8192);
  closed.close();
  assert!(Buffer::new(4096, closed.clone()).is_err());
  assert_eq!(closed.used(), 0);
  drain();
}

#[test]
fn ordinary_cross_thread_release_and_thread_teardown_retire_storage() {
  drain();
  let budget = Budget::new(8192);
  let storage = Buffer::new(4096, budget.clone()).unwrap();
  let other = budget.clone();
  std::thread::spawn(move || {
    drop(storage);
    assert_eq!(pool_retained(), 4096);
    assert_eq!(other.used(), 0);
  })
  .join()
  .unwrap();
  assert_eq!(pool_retained(), 0);
  assert_eq!(budget.used(), 0);
}

#[test]
fn ordinary_retention_remains_bounded_and_does_not_take_receive_entries() {
  drain();
  let budget = Budget::new(8192);
  let receive = Buffer::receive(1024, budget.clone()).unwrap();
  let receive_pointer = address(&receive);
  drop(receive);
  let ordinary = Buffer::new(1024, budget.clone()).unwrap();
  assert_ne!(address(&ordinary), receive_pointer);
  assert_eq!(pool_retained(), 1024);
  let another = Buffer::new(2048, budget.clone()).unwrap();
  drop((ordinary, another));
  assert_eq!(pool_retained(), 4096);
  drop(Buffer::new(4096, budget.clone()).unwrap());
  assert_eq!(pool_retained(), 4096);
  budget.close();
  drain();
  assert_eq!(budget.used(), 0);
}
