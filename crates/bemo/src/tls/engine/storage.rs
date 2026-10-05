/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io;
use std::ops::{Deref, DerefMut};

use compio_buf::{IoBuf, IoBufMut, SetLen};

use crate::buffer::{Budget, Buffer};

/// Retained TLS capacity uses the same allocation accounting as socket buffers.
pub(super) struct Storage {
  buffer: Option<Buffer>,
  budget: Budget,
}

impl Storage {
  pub(super) fn new(budget: Budget) -> Self {
    Self { buffer: None, budget }
  }

  pub(super) fn take(&mut self) -> Self {
    let empty = Self::new(self.budget.clone());
    std::mem::replace(self, empty)
  }

  fn reserve(&mut self, length: usize) -> io::Result<()> {
    if length > super::MAX_RETAINED {
      return Err(io::ErrorKind::OutOfMemory.into());
    }
    let capacity = self.buffer.as_mut().map_or(0, |buffer| buffer.buf_capacity());
    if length > capacity {
      // Charge both allocations while copying; an exhausted owner keeps its original storage.
      let mut next = Buffer::new(length.next_power_of_two().max(256), self.budget.clone())?;
      next.write(0, self)?;
      self.buffer = Some(next);
    }
    Ok(())
  }

  pub(super) fn resize(&mut self, length: usize) -> io::Result<()> {
    self.reserve(length)?;
    if let Some(buffer) = &mut self.buffer {
      let previous = IoBuf::buf_len(buffer);
      if length > previous {
        for byte in &mut buffer.as_uninit()[previous..length] {
          byte.write(0);
        }
      }
      // SAFETY: Existing bytes and the newly zeroed suffix are initialized and within capacity.
      unsafe { buffer.set_len(length) };
    }
    Ok(())
  }

  pub(super) fn extend_from_slice(&mut self, bytes: &[u8]) -> io::Result<()> {
    let start = self.len();
    let length = start.checked_add(bytes.len()).ok_or(io::ErrorKind::OutOfMemory)?;
    self.reserve(length)?;
    if let Some(buffer) = &mut self.buffer {
      buffer.write(start, bytes)?;
    }
    Ok(())
  }

  pub(super) fn truncate(&mut self, length: usize) {
    if let Some(buffer) = &mut self.buffer {
      let length = length.min(IoBuf::buf_len(buffer));
      // SAFETY: Shrinking preserves the initialized prefix.
      unsafe { buffer.set_len(length) };
    }
  }

  pub(super) fn clear(&mut self) {
    self.truncate(0);
  }

  pub(super) fn consume(&mut self, length: usize) {
    let length = length.min(self.len());
    self.copy_within(length.., 0);
    self.truncate(self.len() - length);
  }
}

impl Deref for Storage {
  type Target = [u8];

  fn deref(&self) -> &[u8] {
    self.buffer.as_ref().map_or(&[], |buffer| buffer.as_init())
  }
}

impl DerefMut for Storage {
  fn deref_mut(&mut self) -> &mut [u8] {
    self.buffer.as_mut().map_or(&mut [], |buffer| buffer.as_mut_slice())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn growth_accounts_for_peak_and_preserves_bytes_on_failure() {
    let budget = Budget::new(512);
    let mut storage = Storage::new(budget.clone());
    storage.extend_from_slice(&[7; 256]).unwrap();
    assert_eq!(budget.used(), 256);
    assert!(storage.extend_from_slice(&[8]).is_err());
    assert_eq!(&*storage, &[7; 256]);
    assert_eq!(budget.used(), 256);
    storage.consume(128);
    storage.extend_from_slice(&[9; 128]).unwrap();
    assert_eq!(&storage[..128], &[7; 128]);
    assert_eq!(&storage[128..], &[9; 128]);
    storage.clear();
    assert_eq!(budget.used(), 256);
    storage.resize(256).unwrap();
    assert_eq!(&*storage, &[0; 256]);
    drop(storage);
    assert_eq!(budget.used(), 0);
  }
}
