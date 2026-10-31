//! Panic containment at the process-wide C boundary.
//!
//! A panic may leave shared handle state partially updated. Later entries refuse admission;
//! resources are deliberately retained rather than attempting cleanup through damaged state.
//! Calls admitted before the poison is published may still finish. This is not rollback.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};

// Transport errors -1..-11 and OS errors <= -1000 retain their existing meanings.
const PANIC: i32 = -900;
const POISONED: i32 = -901;

pub(crate) static BOUNDARY: Boundary = Boundary::new();

pub(crate) struct Boundary {
  poisoned: AtomicBool,
}

pub(crate) trait Failure: Sized {
  fn failure(status: i32) -> Self;
}

impl Failure for i32 {
  fn failure(status: i32) -> Self {
    status
  }
}
impl Failure for i64 {
  fn failure(status: i32) -> Self {
    i64::from(status)
  }
}
impl Failure for u32 {
  fn failure(_: i32) -> Self {
    0
  }
}
impl Failure for u64 {
  fn failure(_: i32) -> Self {
    0
  }
}

impl Boundary {
  const fn new() -> Self {
    Self {
      poisoned: AtomicBool::new(false),
    }
  }

  pub(crate) fn call<T: Failure>(&self, action: impl FnOnce() -> T) -> T {
    if self.poisoned.load(Ordering::Acquire) {
      return T::failure(POISONED);
    }
    match catch_unwind(AssertUnwindSafe(action)) {
      Ok(result) => result,
      Err(_) => {
        self.poisoned.store(true, Ordering::Release);
        T::failure(PANIC)
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn preserves_values_and_contains_panic_before_poisoning() {
    let boundary = Boundary::new();
    assert_eq!(boundary.call(|| 42_i32), 42);
    assert_eq!(boundary.call(|| 123_u64), 123);
    assert_eq!(boundary.call::<i32>(|| panic!("boundary probe")), PANIC);
    assert_eq!(boundary.call::<i32>(|| panic!("must never run")), POISONED);
    assert_eq!(boundary.call(|| 1_i64), i64::from(POISONED));
    assert_eq!(boundary.call(|| 1_u64), 0);
    assert_eq!(boundary.call(|| 1_u32), 0);
  }

  #[test]
  fn a_handle_failure_returns_zero_and_poison_is_visible_across_threads() {
    let boundary = Boundary::new();
    assert_eq!(boundary.call::<u64>(|| panic!("handle probe")), 0);
    std::thread::scope(|scope| {
      scope
        .spawn(|| {
          assert_eq!(boundary.call::<i32>(|| panic!("must never run")), POISONED);
        })
        .join()
        .unwrap();
    });
  }

  #[test]
  fn unwinding_drops_owned_temporaries_before_returning() {
    struct Dropped<'a>(&'a AtomicBool);
    impl Drop for Dropped<'_> {
      fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
      }
    }
    let boundary = Boundary::new();
    let dropped = AtomicBool::new(false);
    assert_eq!(
      boundary.call::<i32>(|| {
        let _temporary = Dropped(&dropped);
        panic!("cleanup probe");
      }),
      PANIC
    );
    assert!(dropped.load(Ordering::Relaxed));
  }
}
