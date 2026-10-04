/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Owner-local exchange slots. Wire retirement, not guest release, permits address reuse.

use super::{AddressMap, Budget, Exchange, HttpExchange};
use std::mem::MaybeUninit;

const CACHED_SLOTS: usize = 512;
const CACHED_SPANS: usize = 64;

#[repr(C)]
struct Slot {
  // The foreign-visible layout must remain first and address-stable.
  storage: MaybeUninit<HttpExchange>,
  live: bool,
  quarantined: bool,
  spans: Vec<u32>,
}

impl Drop for Slot {
  fn drop(&mut self) {
    if self.live {
      unsafe { self.storage.assume_init_drop() };
    }
  }
}

#[derive(Default)]
pub(in crate::abi) struct ExchangeStore {
  slots: AddressMap<Box<Slot>>,
  free: Vec<u64>,
}

impl ExchangeStore {
  pub(super) fn insert(&mut self, socket: u64, exchange: Exchange, budget: Budget, stream: u32) -> u64 {
    if let Some(id) = self.free.pop() {
      let slot = self.slots.get_mut(&id).expect("cached exchange slot");
      debug_assert!(!slot.live && !slot.quarantined);
      let spans = std::mem::take(&mut slot.spans);
      slot
        .storage
        .write(HttpExchange::new(socket, exchange, budget, stream, spans));
      slot.live = true;
      // Expose the newly initialized allocation only after all mutable setup is complete.
      return slot.storage.as_ptr() as u64;
    }
    let slot = Box::new(Slot {
      storage: MaybeUninit::new(HttpExchange::new(socket, exchange, budget, stream, Vec::new())),
      live: true,
      quarantined: false,
      spans: Vec::new(),
    });
    let id = slot.storage.as_ptr() as u64;
    self.slots.insert(id, slot);
    // Moving a Box into the registry retags its allocation; expose the installed slot last.
    self.slots.get(&id).unwrap().storage.as_ptr() as u64
  }

  pub(super) fn get(&self, id: &u64) -> Option<&HttpExchange> {
    let slot = self.slots.get(id)?;
    slot.live.then(|| unsafe { slot.storage.assume_init_ref() })
  }

  pub(super) fn get_mut(&mut self, id: &u64) -> Option<&mut HttpExchange> {
    let slot = self.slots.get_mut(id)?;
    slot.live.then(|| unsafe { slot.storage.assume_init_mut() })
  }

  pub(super) fn retire(&mut self, id: u64, quarantine: bool) -> bool {
    let Some(slot) = self.slots.get_mut(&id).filter(|slot| slot.live) else {
      return false;
    };
    let entry = unsafe { slot.storage.assume_init_mut() };
    let mut spans = std::mem::take(&mut entry.spans);
    if spans.capacity() > CACHED_SPANS {
      spans = Vec::new();
    } else {
      spans.clear();
    }
    slot.spans = spans;
    slot.live = false;
    slot.quarantined = quarantine;
    // Only layout storage survives: release every request/response lease immediately.
    unsafe { slot.storage.assume_init_drop() };
    if !quarantine {
      self.cache(id);
    }
    true
  }

  pub(super) fn recycle(&mut self, id: u64) {
    let Some(slot) = self.slots.get_mut(&id).filter(|slot| slot.quarantined) else {
      return;
    };
    slot.quarantined = false;
    self.cache(id);
  }

  fn cache(&mut self, id: u64) {
    if self.free.len() < CACHED_SLOTS {
      self.free.push(id);
    } else {
      self.slots.remove(&id);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn aligned_addresses_distribute_buckets_and_fingerprints() {
    use std::collections::HashSet;
    use std::hash::BuildHasher;

    let store = ExchangeStore::default();
    for stride in [64, 512, 4096] {
      let hashes: Vec<_> = (0..512u64)
        .map(|index| store.slots.hasher().hash_one(0x7fff_0000_0000 + index * stride))
        .collect();
      let buckets: HashSet<_> = hashes.iter().map(|hash| hash & 255).collect();
      let fingerprints: HashSet<_> = hashes.iter().map(|hash| hash >> 57).collect();
      assert!(buckets.len() > 128, "aligned addresses cluster in buckets");
      assert!(fingerprints.len() > 100, "pointer prefixes collapse fingerprints");
    }
  }

  #[test]
  fn idle_slot_cache_has_a_fixed_bound() {
    let mut store = ExchangeStore::default();
    // Retired storage has no live exchange; this exercises the actual wire-retirement cache.
    for id in 1..=(CACHED_SLOTS as u64 + 7) {
      store.slots.insert(
        id,
        Box::new(Slot {
          storage: MaybeUninit::uninit(),
          live: false,
          quarantined: true,
          spans: Vec::new(),
        }),
      );
      store.recycle(id);
    }
    assert_eq!(store.free.len(), CACHED_SLOTS);
    assert_eq!(store.slots.len(), CACHED_SLOTS);
    store.recycle(1);
    assert_eq!(
      store.free.len(),
      CACHED_SLOTS,
      "duplicate retirement cannot duplicate a slot"
    );
  }
}
