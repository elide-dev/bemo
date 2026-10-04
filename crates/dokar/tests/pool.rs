use compio_buf::IoBuf;
use dokar::buffer::{Budget, Buffer, FrozenBuffer, pool_retained, pool_trim};

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
fn only_receive_storage_is_admitted() {
  drain();
  let budget = Budget::new(1024 * 1024);
  drop(Buffer::new(4096, budget.clone()).unwrap());
  assert_eq!(pool_retained(), 0);
  assert_eq!(budget.used(), 0);
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
