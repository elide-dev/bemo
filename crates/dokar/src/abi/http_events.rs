/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Request release must not scan unrelated pipelined requests for absent control events.

use super::{AddressMap, EVENT_BODY_END, EVENT_PART_SENT, EVENT_RESET, NativeEvent, VecDeque};

fn control(event: &NativeEvent) -> bool {
  matches!(event.kind, EVENT_BODY_END | EVENT_PART_SENT | EVENT_RESET)
}

#[derive(Default)]
pub(in crate::abi) struct EventQueue {
  events: VecDeque<NativeEvent>,
  controls: AddressMap<usize>,
}

impl EventQueue {
  fn track(&mut self, event: &NativeEvent) {
    if control(event) {
      *self.controls.entry(event.value).or_default() += 1;
    }
  }

  fn untrack(controls: &mut AddressMap<usize>, event: &NativeEvent) {
    if control(event) {
      let count = controls.get_mut(&event.value).expect("queued control event");
      *count -= 1;
      if *count == 0 {
        controls.remove(&event.value);
      }
    }
  }

  pub(in crate::abi) fn is_empty(&self) -> bool {
    self.events.is_empty()
  }

  pub(in crate::abi) fn front(&self) -> Option<&NativeEvent> {
    self.events.front()
  }

  pub(in crate::abi) fn iter(&self) -> impl Iterator<Item = &NativeEvent> {
    self.events.iter()
  }

  pub(in crate::abi) fn pop_front(&mut self) -> Option<NativeEvent> {
    let event = self.events.pop_front()?;
    Self::untrack(&mut self.controls, &event);
    Some(event)
  }

  pub(in crate::abi) fn push_front(&mut self, event: NativeEvent) {
    self.track(&event);
    self.events.push_front(event);
  }

  pub(in crate::abi) fn push_back(&mut self, event: NativeEvent) {
    self.track(&event);
    self.events.push_back(event);
  }

  pub(in crate::abi) fn retain(&mut self, mut keep: impl FnMut(&NativeEvent) -> bool) {
    let controls = &mut self.controls;
    self.events.retain(|event| {
      let keep = keep(event);
      if !keep {
        Self::untrack(controls, event);
      }
      keep
    });
  }

  pub(in crate::abi) fn forget_exchange(&mut self, exchange: u64) {
    if self.controls.remove(&exchange).is_some() {
      // Remove before slot reuse: a stale event must never name a new exchange at this address.
      self.events.retain(|event| !control(event) || event.value != exchange);
    }
  }
}

impl Extend<NativeEvent> for EventQueue {
  fn extend<T: IntoIterator<Item = NativeEvent>>(&mut self, events: T) {
    for event in events {
      self.push_back(event);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::super::{EVENT_BODY, EVENT_REQUEST, event};
  use super::*;

  #[test]
  fn free_removes_only_that_exchanges_controls_and_preserves_order() {
    let mut queue = EventQueue::default();
    for (kind, exchange) in [
      (EVENT_REQUEST, 1),
      (EVENT_BODY_END, 1),
      (EVENT_BODY, 1),
      (EVENT_PART_SENT, 2),
      (EVENT_RESET, 1),
      (EVENT_REQUEST, 3),
      (EVENT_PART_SENT, 1),
    ] {
      queue.push_back(event(kind, 0, 9, exchange, 0));
    }
    queue.forget_exchange(3);
    queue.forget_exchange(1);
    assert_eq!(
      queue.iter().map(|e| (e.kind, e.value)).collect::<Vec<_>>(),
      vec![
        (EVENT_REQUEST, 1),
        (EVENT_BODY, 1),
        (EVENT_PART_SENT, 2),
        (EVENT_REQUEST, 3)
      ]
    );
    assert_eq!(queue.controls.len(), 1);
    queue.forget_exchange(2);
    assert!(queue.controls.is_empty());
  }

  #[test]
  fn callback_requeue_and_listener_retirement_keep_control_counts_exact() {
    let mut queue = EventQueue::default();
    queue.extend([event(EVENT_BODY_END, 0, 9, 1, 0), event(EVENT_PART_SENT, 0, 9, 1, 0)]);
    let first = queue.pop_front().unwrap();
    assert_eq!(queue.controls[&1], 1);
    queue.push_front(first);
    assert_eq!(queue.controls[&1], 2);
    queue.retain(|e| e.kind != EVENT_BODY_END);
    assert_eq!(queue.controls[&1], 1);
    queue.pop_front().unwrap();
    assert!(queue.controls.is_empty());
    queue.push_back(event(EVENT_RESET, 0, 9, 1, 0));
    queue.forget_exchange(1);
    // Reusing the address has no stale control count or event from its previous lifetime.
    queue.push_back(event(EVENT_REQUEST, 0, 9, 1, 0));
    queue.forget_exchange(1);
    assert_eq!(queue.pop_front().unwrap().kind, EVENT_REQUEST);
    assert!(queue.is_empty());
  }
}
