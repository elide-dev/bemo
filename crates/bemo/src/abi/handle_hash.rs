//! Deterministic mixing for internally minted numeric handles, including table fingerprints.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

pub(super) type HandleMap<V> = HashMap<u64, V, BuildHasherDefault<HandleHasher>>;

#[derive(Default)]
pub(super) struct HandleHasher(u64);

impl Hasher for HandleHasher {
  fn finish(&self) -> u64 {
    self.0
  }

  fn write(&mut self, _: &[u8]) {
    unreachable!("handle registries only hash u64 identities");
  }

  #[inline]
  fn write_u64(&mut self, value: u64) {
    // Keep identity hashing's bucket locality for sequential handles. Multiplication spreads
    // the high seven fingerprint bits without moving low bucket bits or comparing partial keys.
    const FINGERPRINT: u64 = 0x7f << 57;
    let mixed = value.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    self.0 = (value & !FINGERPRINT) | (mixed & FINGERPRINT);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sequential_domain_handles_keep_buckets_and_spread_fingerprints() {
    for domain in [0, 1, 1023, 65535] {
      for stride in [1, 16, 64] {
        let mut buckets = [0usize; 1024];
        let mut fingerprints = [0usize; 128];
        for sequence in 1..=4096u64 {
          let mut hash = HandleHasher::default();
          hash.write_u64((domain << 48) | (sequence * stride));
          let value = hash.finish();
          buckets[value as usize & 1023] += 1;
          fingerprints[(value >> 57) as usize] += 1;
        }
        if stride == 1 {
          assert!(buckets.iter().all(|&count| count == 4));
        }
        // Strides preserve the previous bucket policy; only fingerprints gain distribution.
        assert!(fingerprints.iter().all(|&count| count > 0 && count < 64));
      }
    }
  }

  #[test]
  fn maps_keep_full_identity_and_handle_zero_and_maximum_values() {
    let mut map = HandleMap::default();
    for key in [0, 1, u64::MAX, 1 << 48, (1023 << 48) | 1] {
      assert!(map.insert(key, key).is_none());
      assert_eq!(map.get(&key), Some(&key));
    }
    assert_eq!(map.len(), 5);
    for key in [0, 1, u64::MAX, 1 << 48, (1023 << 48) | 1] {
      assert_eq!(map.remove(&key), Some(key));
      assert!(!map.contains_key(&key));
    }
  }
}
