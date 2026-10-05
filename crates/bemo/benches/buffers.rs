/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

use std::hint::black_box;

use bemo::buffer::{Budget, Buffer, FrozenBuffer};
use compio_buf::IoBuf;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn buffers(c: &mut Criterion) {
  let mut group = c.benchmark_group("buffer");
  for size in [64, 4096, 65536, 1_048_576] {
    let budget = Budget::new(size * 2);
    let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    group.throughput(Throughput::Elements(1));
    group.bench_with_input(BenchmarkId::new("allocate-drop", size), &size, |b, &size| {
      b.iter(|| drop(black_box(Buffer::new(black_box(size), budget.clone()).unwrap())));
    });
    group.throughput(Throughput::Bytes(size as u64));
    group.bench_with_input(BenchmarkId::new("allocate-write-drop", size), &size, |b, &size| {
      b.iter(|| {
        let mut buffer = Buffer::new(size, budget.clone()).unwrap();
        buffer.write(0, black_box(&payload)).unwrap();
        black_box(buffer.as_init());
      });
    });
    {
      let mut buffer = Buffer::new(size, budget.clone()).unwrap();
      buffer.write(0, &payload).unwrap();
      assert_eq!(buffer.as_init(), payload);
      group.bench_function(BenchmarkId::new("overwrite", size), |b| {
        b.iter(|| {
          buffer.write(0, black_box(&payload)).unwrap();
          black_box(buffer.as_init());
        });
      });
      let frozen = buffer.freeze();
      assert_eq!(
        FrozenBuffer::slice(&frozen, 1..size - 1).unwrap().as_ref(),
        &payload[1..size - 1]
      );
      group.throughput(Throughput::Elements(1));
      group.bench_function(BenchmarkId::new("retain-drop", size), |b| {
        b.iter(|| drop(black_box(frozen.clone())));
      });
      group.bench_function(BenchmarkId::new("slice-drop", size), |b| {
        b.iter(|| drop(black_box(FrozenBuffer::slice(&frozen, black_box(1..size - 1)).unwrap())));
      });
      group.bench_function(BenchmarkId::new("shared-mutation-rejected", size), |b| {
        b.iter(|| {
          let result = frozen.clone().try_into_mut();
          assert!(result.is_err());
          black_box(result).unwrap_err();
        });
      });
    }
    assert_eq!(budget.used(), 0);
  }
  group.finish();

  let mut buffer = Some(Buffer::new(4096, Budget::new(4096)).unwrap());
  c.bench_function("buffer/exclusive-freeze-recover", |b| {
    b.iter(|| {
      buffer = Some(black_box(buffer.take().unwrap().freeze()).try_into_mut().unwrap());
      black_box(&buffer);
    });
  });
  c.bench_function("buffer/split-drop", |b| {
    let budget = Budget::new(4096);
    b.iter(|| {
      let buffer = Buffer::new(4096, budget.clone()).unwrap();
      drop(black_box(buffer.split_at(black_box(2048)).unwrap()));
    });
    assert_eq!(budget.used(), 0);
  });
  let denied = Budget::new(1);
  c.bench_function("buffer/budget-rejected", |b| {
    b.iter(|| black_box(Buffer::new(black_box(4096), denied.clone())).unwrap_err());
  });
  assert_eq!(denied.used(), 0);
}

criterion_group!(benches, buffers);
criterion_main!(benches);
