//! Owner-thread application gzip handles, independent of driver or JVM types.

use super::*;
use crate::compression::Gzip;

struct Encoder {
  workload: Workload,
  gzip: Gzip,
}

thread_local! {
  static ENCODERS: RefCell<IntMap<u64, Encoder>> = RefCell::new(IntMap::default());
}

/// Create owner-thread zlib-rs gzip state at level 0..=9; zero rejects invalid admission.
/// Output storage charges workload; backend state and bounded scratch are internal overhead.
pub fn elide_transport_gzip_new(workload: u64, level: u32) -> u64 {
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  let Ok(gzip) = Gzip::new(level) else {
    return 0;
  };
  let id = identity();
  if id != 0 {
    ENCODERS.with(|encoders| {
      encoders.borrow_mut().insert(id, Encoder { workload, gzip });
    });
  }
  id
}

/// Compress frozen input synchronously on the creating thread; return independent frozen output.
/// Reject closed workload, mutable input, invalid handles, or input over 16 MiB with zero.
/// Release output through buffer_release; it remains valid after encoder or input release.
pub fn elide_transport_gzip_compress(workload: u64, encoder: u64, input: u64) -> u64 {
  let input = {
    let buffers = lock(registry(input));
    let Some(Storage::Frozen(buffer)) = buffers.get(&input) else {
      return 0;
    };
    buffer.clone()
  };
  let output = ENCODERS.with(|encoders| {
    let mut encoders = encoders.borrow_mut();
    let encoder = encoders.get_mut(&encoder)?;
    if !encoder.workload.admits(workload) {
      return None;
    }
    encoder.gzip.encode(input.as_ref(), &encoder.workload.budget).ok()
  });
  let Some(output) = output else {
    return 0;
  };
  let id = identity();
  if id != 0 {
    lock(registry(id)).insert(id, Storage::Frozen(output));
  }
  id
}

/// Release owner-thread compressor state; output buffers retain independent ownership.
pub fn elide_transport_gzip_release(encoder: u64) -> i32 {
  ENCODERS.with(|encoders| {
    if encoders.borrow_mut().remove(&encoder).is_some() {
      0
    } else {
      INVALID
    }
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Read;

  #[test]
  fn handles_reject_wrong_thread_workload_and_mutable_input() {
    let owner = elide_transport_owner_new(4096);
    let foreign = elide_transport_owner_new(4096);
    let encoder = elide_transport_gzip_new(owner, 6);
    let input = elide_transport_buffer_new(owner, 4);
    assert_ne!(encoder, 0);
    assert_eq!(elide_transport_gzip_compress(owner, encoder, input), 0);
    // SAFETY: Allocation initialized four bytes; no borrowed mutable view exists.
    assert_eq!(unsafe { elide_transport_buffer_freeze(input, 4) }, 0);
    assert_eq!(elide_transport_gzip_compress(foreign, encoder, input), 0);
    std::thread::spawn(move || {
      assert_eq!(elide_transport_gzip_compress(owner, encoder, input), 0);
      assert_eq!(elide_transport_gzip_release(encoder), INVALID);
    })
    .join()
    .unwrap();
    let output = elide_transport_gzip_compress(owner, encoder, input);
    assert_ne!(output, 0);
    assert_eq!(elide_transport_buffer_release(input), 0);
    assert_eq!(elide_transport_workload_close(owner), 0);
    assert_eq!(elide_transport_gzip_new(owner, 6), 0);
    assert_eq!(elide_transport_gzip_release(encoder), 0);
    assert_eq!(elide_transport_gzip_release(encoder), INVALID);
    {
      let buffers = lock(registry(output));
      let Some(Storage::Frozen(output)) = buffers.get(&output) else {
        panic!("frozen gzip output");
      };
      let mut plaintext = Vec::new();
      flate2::read::GzDecoder::new(output.as_ref())
        .read_to_end(&mut plaintext)
        .unwrap();
      assert_eq!(plaintext, [0; 4]);
    }
    assert_eq!(elide_transport_buffer_release(output), 0);
    assert_eq!(elide_transport_owner_used(owner), 0);
    assert_eq!(elide_transport_owner_release(owner), 0);
    assert_eq!(elide_transport_owner_release(foreign), 0);
  }
}
