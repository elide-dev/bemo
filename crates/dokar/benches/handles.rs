/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

use std::hint::black_box;
use std::mem::MaybeUninit;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use dokar::abi::*;

fn handles(c: &mut Criterion) {
  c.bench_function("owner/create-release", |b| {
    b.iter(|| {
      let owner = elide_transport_owner_new(black_box(65536));
      assert_ne!(owner, 0);
      assert_eq!(elide_transport_owner_release(black_box(owner)), 0);
    });
  });
  let mut group = c.benchmark_group("registry");
  for live in [1, 64, 4096] {
    let owners: Vec<_> = (0..live).map(|_| elide_transport_owner_new(8192)).collect();
    assert!(owners.iter().all(|&id| id != 0));
    let handles: Vec<_> = owners.iter().map(|&id| elide_transport_buffer_new(id, 4096)).collect();
    assert!(handles.iter().all(|&id| id != 0));
    let mut index = 0;
    group.bench_function(BenchmarkId::new("owner-lookup", live), |b| {
      b.iter(|| {
        let owner = owners[index];
        index = (index + 1) % live;
        assert_eq!(black_box(elide_transport_owner_used(black_box(owner))), 4096);
      });
    });
    group.bench_function(BenchmarkId::new("buffer-view", live), |b| {
      b.iter(|| {
        let handle = handles[index];
        index = (index + 1) % live;
        let mut view = MaybeUninit::<BufferView>::uninit();
        // This thread owns the live handle; output is aligned writable storage.
        assert_eq!(
          // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
          unsafe { elide_transport_buffer_view(black_box(handle), view.as_mut_ptr()) },
          0
        );
        // SAFETY: buffer_view succeeded and initialized every field of the output.
        let view = unsafe { view.assume_init() };
        assert_eq!(view.capacity, 4096);
        black_box(view);
      });
    });
    group.bench_function(BenchmarkId::new("allocate-release", live), |b| {
      b.iter(|| {
        let owner = owners[index];
        index = (index + 1) % live;
        let handle = elide_transport_buffer_new(black_box(owner), 4096);
        assert_ne!(handle, 0);
        assert_eq!(elide_transport_buffer_release(black_box(handle)), 0);
      });
    });
    for &handle in &handles {
      // No foreign views or writers survive the buffer-view iteration.
      // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
      assert_eq!(unsafe { elide_transport_buffer_freeze(handle, 4096) }, 0);
    }
    group.bench_function(BenchmarkId::new("slice-release", live), |b| {
      b.iter(|| {
        let handle = handles[index];
        index = (index + 1) % live;
        let slice = elide_transport_buffer_slice(black_box(handle), 16, 1024);
        assert_ne!(slice, 0);
        assert_eq!(elide_transport_buffer_release(black_box(slice)), 0);
      });
    });
    for handle in handles {
      assert_eq!(elide_transport_buffer_release(handle), 0);
    }
    for owner in owners {
      assert_eq!(elide_transport_owner_used(owner), 0);
      assert_eq!(elide_transport_owner_release(owner), 0);
    }
  }
  group.finish();
}

criterion_group!(benches, handles);
criterion_main!(benches);
