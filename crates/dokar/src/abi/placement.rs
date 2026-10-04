/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Thread placement is established before allocating a shard's reactor, then restored at release.

#[cfg(any(target_os = "linux", test))]
mod locality;

use std::io;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

struct MaskNode {
  references: AtomicUsize,
  next: AtomicPtr<MaskNode>,
  words: Vec<usize>,
}

impl MaskNode {
  #[cfg(any(target_os = "linux", test))]
  fn new(words: Vec<usize>) -> *mut Self {
    Box::into_raw(Box::new(Self {
      references: AtomicUsize::new(1),
      next: AtomicPtr::new(ptr::null_mut()),
      words,
    }))
  }
}

static RETIRED_MASKS: AtomicPtr<MaskNode> = AtomicPtr::new(ptr::null_mut());

/// # Safety
/// `mask` must carry a live reference, held until this call returns.
#[cfg(any(target_os = "linux", test))]
unsafe fn retain_mask(mask: *mut MaskNode) -> *mut MaskNode {
  if !mask.is_null() {
    // The owner reference prevents retirement while the parent captures its child reference.
    unsafe { (*mask).references.fetch_add(1, Ordering::Relaxed) };
  }
  mask
}

/// # Safety
/// Consume exactly one owned reference, or null. Never pass an already-consumed reference.
unsafe fn release_mask(mask: *mut MaskNode) {
  if mask.is_null() || unsafe { (*mask).references.fetch_sub(1, Ordering::AcqRel) } != 1 {
    return;
  }
  // The SVM exit hook cannot allocate, free, lock, or issue syscalls. Publish for later reclamation.
  let mut head = RETIRED_MASKS.load(Ordering::Relaxed);
  loop {
    unsafe { (*mask).next.store(head, Ordering::Relaxed) };
    match RETIRED_MASKS.compare_exchange_weak(head, mask, Ordering::Release, Ordering::Relaxed) {
      Ok(_) => break,
      Err(current) => head = current,
    }
  }
}

fn reclaim_masks() {
  let mut mask = RETIRED_MASKS.swap(ptr::null_mut(), Ordering::Acquire);
  while !mask.is_null() {
    // Only zero-reference nodes are published; exchanging the list gives this thread ownership.
    let node = unsafe { Box::from_raw(mask) };
    mask = node.next.load(Ordering::Relaxed);
  }
}

pub(super) fn helper_prepare() -> u64 {
  #[cfg(target_os = "linux")]
  {
    PINNED.with(|mask| unsafe { retain_mask(mask.get()) } as u64)
  }
  #[cfg(not(target_os = "linux"))]
  0
}

/// # Safety
/// `token` must be zero or an unconsumed token from `helper_prepare`.
pub(super) unsafe fn helper_release(token: u64) {
  unsafe { release_mask(token as *mut MaskNode) };
}

/// # Safety
/// `token` must be zero or an unconsumed token from `helper_prepare`.
/// # Errors
/// Returns the OS error if affinity restoration fails; the token is consumed even on failure.
pub(super) unsafe fn helper_consume(token: u64) -> io::Result<()> {
  let node = token as *mut MaskNode;
  let result = if node.is_null() {
    Ok(())
  } else {
    helper_start(Some(unsafe { &(*node).words }))
  };
  unsafe { release_mask(node) };
  reclaim_masks();
  result
}

#[cfg(any(target_os = "linux", test))]
fn physical_cores(
  allowed: &[usize],
  mut siblings: impl FnMut(usize) -> io::Result<String>,
) -> io::Result<Vec<Vec<usize>>> {
  let mut cores: Vec<Vec<usize>> = Vec::new();
  for &cpu in allowed {
    if cores.iter().any(|core| core.contains(&cpu)) {
      continue;
    }
    let mut core = Vec::new();
    for range in siblings(cpu)?.trim().split(',') {
      let (first, last) = range.split_once('-').unwrap_or((range, range));
      let first: usize = first.parse().map_err(|_| io::ErrorKind::InvalidData)?;
      let last: usize = last.parse().map_err(|_| io::ErrorKind::InvalidData)?;
      if first > last {
        return Err(io::ErrorKind::InvalidData.into());
      }
      core.extend(allowed.iter().copied().filter(|cpu| (first..=last).contains(cpu)));
    }
    core.sort_unstable();
    core.dedup();
    if !core.contains(&cpu) || cores.iter().any(|prior| prior.iter().any(|cpu| core.contains(cpu))) {
      return Err(io::ErrorKind::InvalidData.into());
    }
    cores.push(core);
  }
  Ok(cores)
}

pub(super) fn serving_cores(cores: Vec<Vec<usize>>, contexts: usize, workers: usize) -> Vec<Vec<usize>> {
  #[cfg(target_os = "linux")]
  {
    locality::assign(cores, contexts, workers)
  }
  #[cfg(not(target_os = "linux"))]
  {
    let _ = (contexts, workers);
    cores
  }
}

pub(super) fn topology() -> io::Result<Vec<Vec<usize>>> {
  #[cfg(target_os = "linux")]
  {
    physical_cores(&allowed()?, |cpu| {
      std::fs::read_to_string(format!(
        "/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list"
      ))
    })
  }
  #[cfg(not(target_os = "linux"))]
  Ok(vec![Vec::new()])
}

pub(super) fn sibling_placements(cores: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
  let mut slots = Vec::with_capacity(cores.iter().map(Vec::len).sum());
  for sibling in 0..cores.iter().map(Vec::len).max().unwrap_or(0) {
    for core in &cores {
      if let Some(&cpu) = core.get(sibling) {
        slots.push(vec![cpu]);
      }
    }
  }
  slots
}

#[cfg(target_os = "linux")]
thread_local! {
  static PINNED: std::cell::Cell<*mut MaskNode> = const { std::cell::Cell::new(ptr::null_mut()) };
}

#[cfg(target_os = "linux")]
fn mask() -> io::Result<Vec<usize>> {
  let mut words = vec![0usize; 16];
  loop {
    // The kernel accepts a variable-length, word-aligned mask, including machines above 1024 CPUs.
    let result =
      unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(words.as_slice()), words.as_mut_ptr().cast()) };
    if result == 0 {
      return Ok(words);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::EINVAL) {
      return Err(error);
    }
    let length = words.len().checked_mul(2).ok_or(io::ErrorKind::OutOfMemory)?;
    words
      .try_reserve_exact(length - words.len())
      .map_err(|_| io::ErrorKind::OutOfMemory)?;
    words.resize(length, 0);
  }
}

#[cfg(target_os = "linux")]
pub(super) fn allowed() -> io::Result<Vec<usize>> {
  #[cfg(target_os = "linux")]
  {
    let allowed = mask()?;
    let cpus: Vec<_> = allowed
      .iter()
      .enumerate()
      .flat_map(|(word, mask)| {
        (0..usize::BITS as usize)
          .filter_map(move |bit| (mask & (1usize << bit) != 0).then_some(word * usize::BITS as usize + bit))
      })
      .collect();
    if cpus.is_empty() {
      return Err(io::ErrorKind::NotFound.into());
    }
    Ok(cpus)
  }
  #[cfg(not(target_os = "linux"))]
  Ok(Vec::new())
}

#[cfg(all(test, target_os = "linux"))]
fn helper_mask() -> Option<Vec<usize>> {
  PINNED.with(|mask| unsafe { mask.get().as_ref() }.map(|node| node.words.clone()))
}

pub(super) fn helper_start(allowed: Option<&[usize]>) -> io::Result<()> {
  #[cfg(not(target_os = "linux"))]
  let _ = allowed;
  #[cfg(target_os = "linux")]
  if PINNED.with(|mask| mask.get().is_null())
    && let Some(allowed) = allowed
    && unsafe { libc::sched_setaffinity(0, std::mem::size_of_val(allowed), allowed.as_ptr().cast()) } != 0
  {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

pub(super) struct Placement {
  pub(super) cpu: Option<usize>,
  #[cfg(target_os = "linux")]
  previous: Vec<usize>,
}

impl Placement {
  pub(super) fn pin(cpu: Option<usize>) -> io::Result<Self> {
    reclaim_masks();
    #[cfg(target_os = "linux")]
    {
      if PINNED.with(|mask| !mask.get().is_null()) {
        return Err(io::ErrorKind::AlreadyExists.into());
      }
      let previous = mask()?;
      if let Some(cpu) = cpu {
        let word = cpu / usize::BITS as usize;
        let bit = 1usize << (cpu % usize::BITS as usize);
        if previous.get(word).is_none_or(|value| value & bit == 0) {
          return Err(io::ErrorKind::PermissionDenied.into());
        }
        let mut selected = vec![0usize; previous.len()];
        selected[word] = bit;
        if unsafe { libc::sched_setaffinity(0, std::mem::size_of_val(selected.as_slice()), selected.as_ptr().cast()) }
          != 0
        {
          return Err(io::Error::last_os_error());
        }
        PINNED.with(|mask| mask.set(MaskNode::new(previous.clone())));
      }
      Ok(Self { cpu, previous })
    }
    #[cfg(not(target_os = "linux"))]
    Ok(Self { cpu })
  }

  pub(super) fn restore(&mut self) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if self.cpu.is_some() {
      if unsafe {
        libc::sched_setaffinity(
          0,
          std::mem::size_of_val(self.previous.as_slice()),
          self.previous.as_ptr().cast(),
        )
      } != 0
      {
        return Err(io::Error::last_os_error());
      }
      PINNED.with(|mask| unsafe { release_mask(mask.replace(ptr::null_mut())) });
      self.cpu = None;
    }
    reclaim_masks();
    Ok(())
  }
}

impl Drop for Placement {
  fn drop(&mut self) {
    if self.restore().is_err() {
      #[cfg(target_os = "linux")]
      PINNED.with(|mask| unsafe { release_mask(mask.replace(ptr::null_mut())) });
      reclaim_masks();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sibling_slots_preserve_sparse_masks_and_uneven_core_widths() {
    assert_eq!(
      sibling_placements(vec![vec![8, 20], vec![2], vec![7, 19, 31]]),
      vec![vec![8], vec![2], vec![7], vec![20], vec![19], vec![31]]
    );
    assert_eq!(
      sibling_placements(vec![vec![20], vec![2], vec![19]]),
      vec![vec![20], vec![2], vec![19]]
    );
    assert!(sibling_placements(Vec::new()).is_empty());
  }

  #[test]
  fn helper_token_outlives_owner_and_can_retire_on_another_thread() {
    let owner = MaskNode::new(vec![0x28, 0x400]);
    let child = unsafe { retain_mask(owner) } as usize;
    unsafe { release_mask(owner) };
    std::thread::spawn(move || {
      let child = child as *mut MaskNode;
      assert_eq!(unsafe { &(*child).words }, &[0x28, 0x400]);
      unsafe { release_mask(child) };
    })
    .join()
    .unwrap();
    reclaim_masks();
  }

  #[test]
  fn concurrent_helper_releases_preserve_the_live_owner() {
    let owner = MaskNode::new(vec![0x180]);
    let children: Vec<_> = (0..32)
      .map(|_| {
        let token = unsafe { retain_mask(owner) } as usize;
        std::thread::spawn(move || unsafe { release_mask(token as *mut MaskNode) })
      })
      .collect();
    for child in children {
      child.join().unwrap();
    }
    reclaim_masks();
    assert_eq!(
      unsafe { (*owner).references.load(std::sync::atomic::Ordering::Acquire) },
      1
    );
    assert_eq!(unsafe { &(*owner).words }, &[0x180]);
    unsafe { release_mask(owner) };
    reclaim_masks();
  }

  #[test]
  fn concurrent_retirement_and_reclamation_keep_captured_masks_live() {
    let workers: Vec<_> = (0..4)
      .map(|worker| {
        std::thread::spawn(move || {
          for index in 0..64 {
            let owner = MaskNode::new(vec![worker, index]);
            let child = unsafe { retain_mask(owner) };
            unsafe { release_mask(owner) };
            reclaim_masks();
            assert_eq!(unsafe { &(*child).words }, &[worker, index]);
            unsafe { release_mask(child) };
            reclaim_masks();
          }
        })
      })
      .collect();
    for worker in workers {
      worker.join().unwrap();
    }
    reclaim_masks();
  }

  #[test]
  fn sparse_allowed_siblings_are_grouped_by_physical_core() {
    let cores = physical_cores(&[3, 8, 11, 19], |cpu| {
      Ok(
        match cpu {
          3 | 11 => "3,11",
          8 => "0,8",
          19 => "19-20",
          _ => unreachable!(),
        }
        .to_owned(),
      )
    })
    .unwrap();
    assert_eq!(cores, vec![vec![3, 11], vec![8], vec![19]]);
  }

  #[test]
  fn malformed_or_inconsistent_sibling_lists_are_rejected() {
    let parse =
      |allowed: &[usize], list: &'static str| physical_cores(allowed, |_| Ok(list.to_owned())).unwrap_err().kind();
    assert_eq!(parse(&[0], "x"), io::ErrorKind::InvalidData);
    assert_eq!(parse(&[0], "0-y"), io::ErrorKind::InvalidData);
    assert_eq!(parse(&[0], "1-0"), io::ErrorKind::InvalidData);
    assert_eq!(
      parse(&[0], "1"),
      io::ErrorKind::InvalidData,
      "a core must contain its CPU"
    );
    let overlapping = physical_cores(&[0, 1, 2], |cpu| Ok(if cpu == 0 { "0-1" } else { "1-2" }.to_owned()));
    assert_eq!(overlapping.unwrap_err().kind(), io::ErrorKind::InvalidData);
    let missing = physical_cores(&[0], |_| Err(io::ErrorKind::NotFound.into()));
    assert_eq!(missing.unwrap_err().kind(), io::ErrorKind::NotFound);
    assert!(physical_cores(&[], |_| unreachable!()).unwrap().is_empty());
  }

  #[cfg(target_os = "linux")]
  #[test]
  #[cfg_attr(miri, ignore = "sched_getaffinity is unsupported under miri")]
  fn pinning_rejects_disallowed_cpus_and_nested_owners() {
    std::thread::spawn(|| {
      let error = Placement::pin(Some(1 << 20))
        .err()
        .expect("CPU outside the allowed mask");
      assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
      let mut unpinned = Placement::pin(None).unwrap();
      assert_eq!(unpinned.cpu, None);
      unpinned.restore().unwrap();
      let cpus = allowed().unwrap();
      let mut owner = Placement::pin(Some(cpus[0])).unwrap();
      let nested = Placement::pin(Some(cpus[0])).err().expect("one owner per thread");
      assert_eq!(nested.kind(), io::ErrorKind::AlreadyExists);
      owner.restore().unwrap();
      assert_eq!(owner.cpu, None);
      assert!(helper_mask().is_none());
    })
    .join()
    .unwrap();
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn helper_start_restores_allowed_cpus_without_unpinning_owner() {
    let cpus = allowed().unwrap();
    let original = mask().unwrap();
    let placement = Placement::pin(Some(cpus[0])).unwrap();
    let pinned = mask().unwrap();
    let captured = helper_mask().unwrap();
    helper_start(Some(&captured)).unwrap();
    assert_eq!(mask().unwrap(), pinned);
    let inherited = std::thread::spawn(move || {
      assert_eq!(mask().unwrap(), pinned);
      helper_start(Some(&captured)).unwrap();
      mask().unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(inherited, original);
    drop(placement);
    assert_eq!(mask().unwrap(), original);
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn topology_queries_cannot_change_another_owners_helper_mask() {
    let cpus = allowed().unwrap();
    if cpus.len() < 2 {
      return;
    }
    let original = mask().unwrap();
    let temporary = Placement::pin(Some(cpus[0])).unwrap();
    topology().unwrap();
    drop(temporary);
    let owner = Placement::pin(Some(cpus[1])).unwrap();
    let captured = helper_mask().unwrap();
    let restored = std::thread::spawn(move || {
      helper_start(Some(&captured)).unwrap();
      mask().unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(restored, original);
    drop(owner);
    assert!(helper_mask().is_none());
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn helper_restoration_does_not_expand_a_later_restricted_mask() {
    let cpus = allowed().unwrap();
    topology().unwrap();
    std::thread::spawn(move || {
      let mut restricted = mask().unwrap();
      restricted.fill(0);
      restricted[cpus[0] / usize::BITS as usize] = 1 << (cpus[0] % usize::BITS as usize);
      helper_start(Some(&restricted)).unwrap();
      let owner = Placement::pin(Some(cpus[0])).unwrap();
      let captured = helper_mask().unwrap();
      let restored = std::thread::spawn(move || {
        helper_start(Some(&captured)).unwrap();
        mask().unwrap()
      })
      .join()
      .unwrap();
      assert_eq!(restored, restricted);
      drop(owner);
    })
    .join()
    .unwrap();
  }
}
