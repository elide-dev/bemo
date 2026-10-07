//! Reusable application gzip; native output owns an independent frozen allocation.

use crate::buffer::{Budget, Buffer, FrozenBuffer};
use std::io;
use zlib_rs::{Deflate, DeflateFlush, Status};

/// Maximum uncompressed member size; bounds retained compressor scratch per instance.
pub const MAX_INPUT: usize = 16 * 1024 * 1024;

/// Thread-confined reusable raw deflate state and bounded scratch storage.
/// Scratch and backend state are internal overhead; returned storage charges the caller budget.
pub struct Gzip {
  compressor: Deflate,
  scratch: Vec<u8>,
}

impl Gzip {
  /// Create a zlib-rs encoder at level 0..=9.
  pub fn new(level: u32) -> io::Result<Self> {
    if level > 9 {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(Self {
      compressor: Deflate::new(level as i32, false, 15),
      scratch: Vec::new(),
    })
  }

  /// Reset and compress fresh input into a complete independent gzip member.
  /// Input larger than [`MAX_INPUT`] is rejected before growing scratch.
  pub fn encode(&mut self, input: &[u8], budget: &Budget) -> io::Result<FrozenBuffer> {
    if input.len() > MAX_INPUT || budget.is_closed() {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let bound = input.len() + input.len() / 8 + 1024;
    if self.scratch.len() < bound {
      self
        .scratch
        .try_reserve(bound - self.scratch.len())
        .map_err(|_| io::ErrorKind::OutOfMemory)?;
      self.scratch.resize(bound, 0);
    }
    self.compressor.reset();
    self.scratch[..10].copy_from_slice(&[31, 139, 8, 0, 0, 0, 0, 0, 0, 255]);
    let status = self
      .compressor
      .compress(input, &mut self.scratch[10..bound - 8], DeflateFlush::Finish)
      .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.as_str()))?;
    if status != Status::StreamEnd || self.compressor.total_in() != input.len() as u64 {
      return Err(io::ErrorKind::InvalidData.into());
    }
    let end = 10 + self.compressor.total_out() as usize;
    self.scratch[end..end + 4].copy_from_slice(&crc32fast::hash(input).to_le_bytes());
    self.scratch[end + 4..end + 8].copy_from_slice(&(input.len() as u32).to_le_bytes());
    let mut output = Buffer::new(end + 8, budget.clone())?;
    output.write(0, &self.scratch[..end + 8])?;
    Ok(output.freeze())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Read;

  #[test]
  fn changing_members_are_independent_and_budgeted() {
    let budget = Budget::new(1024 * 1024);
    let mut encoder = Gzip::new(6).unwrap();
    let size = if cfg!(miri) { 64 } else { 65536 };
    let repeated = vec![42; size];
    let random: Vec<u8> = (0..size).map(|i| ((i * 17 + i / 257) % 251) as u8).collect();
    let mut held = Vec::new();
    for input in [&random[..], &[][..], b"changing", &repeated[..]] {
      let output = encoder.encode(input, &budget).unwrap();
      let mut decoded = Vec::new();
      flate2::read::GzDecoder::new(output.as_ref())
        .read_to_end(&mut decoded)
        .unwrap();
      assert_eq!(decoded, input);
      held.push(output);
    }
    drop(encoder);
    assert_eq!(
      budget.used(),
      held.iter().map(|part| part.as_ref().len()).sum::<usize>()
    );
    drop(held);
    assert_eq!(budget.used(), 0);
  }

  #[test]
  fn invalid_level_and_exhausted_output_are_rejected() {
    assert!(Gzip::new(10).is_err());
    let mut encoder = Gzip::new(6).unwrap();
    let budget = Budget::new(1);
    assert_eq!(
      encoder.encode(b"test", &budget).unwrap_err().kind(),
      io::ErrorKind::OutOfMemory
    );
    assert_eq!(budget.used(), 0);
    if !cfg!(miri) {
      assert!(encoder.encode(&vec![0; MAX_INPUT + 1], &budget).is_err());
    }
  }
}
