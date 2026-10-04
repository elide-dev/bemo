/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Immutable send leases survive ordinary completion until the kernel releases them.

use super::*;

pub(super) const THRESHOLD: usize = 16 * 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_SENDS: usize = 128;

struct Lease {
  _buffers: Vec<FrozenBuffer>,
  bytes: usize,
  initial: bool,
  notified: bool,
}

#[derive(Default)]
pub(super) struct Sends {
  leases: IntMap<u64, Lease>,
  bytes: usize,
}

impl Sends {
  pub(super) fn bytes(&self) -> usize {
    self.bytes
  }

  pub(super) fn retain(&mut self, token: u64, views: &[FrozenBuffer]) -> bool {
    let Some(bytes) = views
      .iter()
      .try_fold(0usize, |total, view| total.checked_add(view.retained_capacity()))
    else {
      return false;
    };
    // Count shared backing conservatively: a small slice may retain a much larger allocation.
    if self.leases.len() >= MAX_SENDS || bytes > MAX_BYTES - self.bytes {
      return false;
    }
    assert!(!self.leases.contains_key(&token));
    self.leases.insert(
      token,
      Lease {
        _buffers: views.to_vec(),
        bytes,
        initial: false,
        notified: false,
      },
    );
    self.bytes += bytes;
    true
  }

  pub(super) fn abandon(&mut self, token: u64) {
    let lease = self.leases.remove(&token).expect("reserved send lease");
    self.bytes -= lease.bytes;
  }

  /// Returns true only when both send result and any promised notification have retired.
  pub(super) fn initial(&mut self, token: u64, more: bool) -> io::Result<bool> {
    let lease = self.leases.get_mut(&token).ok_or(io::ErrorKind::InvalidData)?;
    if lease.initial || (!more && lease.notified) {
      return Err(io::ErrorKind::InvalidData.into());
    }
    lease.initial = true;
    let retired = !more || lease.notified;
    if retired {
      self.abandon(token);
    }
    Ok(retired)
  }

  pub(super) fn notification(&mut self, token: u64) -> io::Result<bool> {
    let lease = self.leases.get_mut(&token).ok_or(io::ErrorKind::InvalidData)?;
    if lease.notified {
      return Err(io::ErrorKind::InvalidData.into());
    }
    lease.notified = true;
    let retired = lease.initial;
    if retired {
      self.abandon(token);
    }
    Ok(retired)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn view(budget: &Budget, capacity: usize) -> FrozenBuffer {
    let mut buffer = Buffer::new(capacity, budget.clone()).unwrap();
    buffer.write(0, b"payload").unwrap();
    buffer.freeze()
  }

  #[test]
  fn send_result_keeps_storage_immutable_until_notification() {
    let budget = Budget::new(4096);
    let buffer = view(&budget, 4096);
    let mut sends = Sends::default();
    assert!(sends.retain(1, std::slice::from_ref(&buffer)));
    assert!(!sends.initial(1, true).unwrap());
    let buffer = buffer.try_into_mut().unwrap_err();
    drop(buffer);
    assert_eq!(budget.used(), 4096);
    assert!(sends.notification(1).unwrap());
    assert_eq!(sends.bytes(), 0);
    assert_eq!(budget.used(), 0);
    assert!(sends.notification(1).is_err());
  }

  #[test]
  fn short_send_and_later_send_retain_independent_notifications() {
    let budget = Budget::new(4096);
    let buffer = view(&budget, 4096);
    let mut sends = Sends::default();
    assert!(sends.retain(1, std::slice::from_ref(&buffer)));
    assert!(!sends.initial(1, true).unwrap());
    let tail = FrozenBuffer::slice(&buffer, 3..7).unwrap();
    assert!(sends.retain(2, &[tail]));
    assert!(!sends.initial(2, true).unwrap());
    drop(buffer);
    assert_eq!(sends.bytes(), 8192);
    assert!(sends.notification(2).unwrap());
    assert_eq!(budget.used(), 4096);
    assert!(sends.notification(1).unwrap());
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn early_notification_and_terminal_result_release_once() {
    let budget = Budget::new(4096);
    let buffer = view(&budget, 4096);
    let mut sends = Sends::default();
    assert!(sends.retain(1, std::slice::from_ref(&buffer)));
    assert!(!sends.notification(1).unwrap());
    assert!(sends.notification(1).is_err());
    assert!(sends.initial(1, true).unwrap());
    assert!(sends.initial(1, true).is_err());
    assert!(sends.retain(2, std::slice::from_ref(&buffer)));
    assert!(sends.initial(2, false).unwrap());
    assert!(buffer.try_into_mut().is_ok());
  }

  #[test]
  fn pending_limits_count_full_backing_and_send_count() {
    let budget = Budget::new(MAX_BYTES);
    let buffer = view(&budget, MAX_BYTES);
    let mut sends = Sends::default();
    assert!(sends.retain(1, std::slice::from_ref(&buffer)));
    assert_eq!(sends.bytes(), MAX_BYTES);
    assert!(!sends.retain(2, std::slice::from_ref(&buffer)));
    sends.abandon(1);
    assert_eq!(sends.bytes(), 0);
    drop(buffer);
    let buffer = view(&budget, THRESHOLD);
    for token in 0..MAX_SENDS as u64 {
      assert!(sends.retain(token, std::slice::from_ref(&buffer)));
    }
    assert!(!sends.retain(MAX_SENDS as u64, &[buffer]));
  }
}
