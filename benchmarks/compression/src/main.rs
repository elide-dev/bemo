//! Standalone backend probe. This does not change the JVM's compression provider.

use std::hint::black_box;
use std::io::Read;
use std::time::Instant;

#[cfg(not(feature = "zlib-rs"))]
use flate2::{Compress, Compression, FlushCompress, Status};
#[cfg(feature = "zlib-rs")]
use zlib_rs::{Deflate as Compress, DeflateFlush as FlushCompress, Status};

#[cfg(not(any(feature = "zlib", feature = "zlib-rs", feature = "zlib-ng")))]
compile_error!("Select exactly one compression backend");
#[cfg(any(
  all(feature = "zlib", feature = "zlib-rs"),
  all(feature = "zlib", feature = "zlib-ng"),
  all(feature = "zlib-rs", feature = "zlib-ng")
))]
compile_error!("Select exactly one compression backend");

fn encode(compressor: &mut Compress, input: &[u8], output: &mut [u8]) -> usize {
  compressor.reset();
  output[..10].copy_from_slice(&[31, 139, 8, 0, 0, 0, 0, 0, 0, 255]);
  let status = compressor
    .compress(input, &mut output[10..], FlushCompress::Finish)
    .unwrap();
  assert_eq!(status, Status::StreamEnd, "output bound was too small");
  assert_eq!(compressor.total_in(), input.len() as u64);
  let end = 10 + compressor.total_out() as usize;
  output[end..end + 4].copy_from_slice(&crc32fast::hash(input).to_le_bytes());
  output[end + 4..end + 8].copy_from_slice(&(input.len() as u32).to_le_bytes());
  end + 8
}

#[cfg(feature = "zlib-rs")]
fn checksums(input: &[u8], random: bool, iterations: usize) {
  let size = input.len();
  let expected = crc32fast::hash(input);
  for (algorithm, hash) in [
    ("crc32fast", crc32fast::hash as fn(&[u8]) -> u32),
    (
      "zlib-rs",
      (|bytes: &[u8]| zlib_rs::crc32::crc32(0, bytes)) as fn(&[u8]) -> u32,
    ),
  ] {
    assert_eq!(hash(input), expected);
    for _ in 0..1000 {
      black_box(hash(black_box(input)));
    }
    for sample in 0..3 {
      let start = Instant::now();
      for _ in 0..iterations {
        black_box(hash(black_box(input)));
      }
      let nanos = start.elapsed().as_nanos() as f64 / iterations as f64;
      println!(
        "{{\"kind\":\"checksum\",\"algorithm\":\"{algorithm}\",\"size\":{size},\"random\":{random},\"sample\":{sample},\"ns_per_checksum\":{nanos}}}"
      );
    }
  }
}

fn main() {
  let iterations: usize = std::env::args()
    .nth(1)
    .unwrap_or_else(|| "10000".into())
    .parse()
    .unwrap();
  assert!(iterations > 0);
  let backend = if cfg!(feature = "zlib-rs") {
    "zlib-rs"
  } else if cfg!(feature = "zlib-ng") {
    "zlib-ng"
  } else {
    "zlib"
  };
  let levels: Vec<u32> = std::env::args()
    .nth(2)
    .unwrap_or_else(|| "6".into())
    .split(',')
    .map(|value| value.parse().unwrap())
    .collect();
  assert!(!levels.is_empty() && levels.iter().all(|level| *level <= 9));
  for size in [1024, 65536, 131072] {
    for random in [false, true] {
      let pattern = b"{\"message\":\"bemo transport benchmark\",\"value\":12345}\n";
      let mut state = 17u64;
      let input: Vec<u8> = (0..size)
        .map(|i| {
          if random {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
          } else {
            pattern[i % pattern.len()]
          }
        })
        .collect();
      #[cfg(feature = "zlib-rs")]
      checksums(&input, random, iterations);
      for level in &levels {
        #[cfg(not(feature = "zlib-rs"))]
        let mut compressor = Compress::new(Compression::new(*level), false);
        #[cfg(feature = "zlib-rs")]
        let mut compressor = Compress::new(*level as i32, false, 15);
        let mut output = vec![0; size + size / 8 + 1024];
        let length = encode(&mut compressor, &input, &mut output);
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(&output[..length])
          .read_to_end(&mut decoded)
          .unwrap();
        assert_eq!(decoded, input);
        for _ in 0..1000 {
          black_box(encode(&mut compressor, black_box(&input), &mut output));
        }
        for sample in 0..3 {
          let start = Instant::now();
          for _ in 0..iterations {
            black_box(encode(&mut compressor, black_box(&input), black_box(&mut output)));
          }
          let nanos = start.elapsed().as_nanos() as f64 / iterations as f64;
          println!(
            "{{\"backend\":\"{backend}\",\"size\":{size},\"random\":{random},\"sample\":{sample},\"level\":{level},\"compressed_bytes\":{length},\"ns_per_member\":{nanos}}}"
          );
        }
      }
    }
  }
}
