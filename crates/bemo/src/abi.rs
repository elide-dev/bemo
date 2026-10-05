/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Versioned C ABI shared by FFM and Native Image. Handles never reuse an identity.
//!
//! Every entry that starts work takes a leading `workload`: an owner handle from
//! [`elide_transport_owner_new`]. Work on an existing socket must name the workload that created it,
//! and closing a workload cancels its operations without touching other workloads.

use nohash_hasher::IntMap;
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::task::Waker;

use crate::buffer::{Budget, Buffer, FrozenBuffer};
use crate::driver::{Backend, Driver};
use compio_buf::{IoBuf, IoBufMut, SetLen};

mod tls;
pub use tls::*;

mod socket;
pub use socket::*;

mod http;
pub use http::*;

mod serving;
pub use serving::*;
pub mod engine;
mod placement;

/// Invalid handle, argument, or ownership state.
pub const INVALID: i32 = -1;
/// Driver cancellation has not yet finished; retry release on the owner thread.
pub const BUSY: i32 = -2;
/// Operation results at or below `-OS_ERROR_BASE` carry an unmapped OS error:
/// `errno = -result - OS_ERROR_BASE`. `-3` remains for failures without an errno.
pub const OS_ERROR_BASE: i64 = 1000;
/// View is immutable.
pub const READ_ONLY: u64 = 1;

/// Stable native buffer descriptor. The handle, not this address, owns its lifetime.
#[repr(C)]
#[derive(Default)]
pub struct BufferView {
  /// Borrowed address; mutable only when flags are zero.
  pub address: *mut c_void,
  /// Accessible capacity, restricted to initialized bytes for frozen views.
  pub capacity: u64,
  /// Initialized logical length.
  pub length: u64,
  /// READ_ONLY for a frozen view.
  pub flags: u64,
}

enum Storage {
  Mutable(Buffer),
  Frozen(FrozenBuffer),
}

// Only internally generated integer identities are inserted into these maps.
static NEXT: AtomicU64 = AtomicU64::new(1);
static OWNERS: LazyLock<Mutex<IntMap<u64, Budget>>> = LazyLock::new(Mutex::default);
static WAKERS: LazyLock<Mutex<IntMap<u64, Waker>>> = LazyLock::new(Mutex::default);
/// Incremented after every workload closure; drivers sweep closed workloads when it moves.
static CLOSURES: AtomicU64 = AtomicU64::new(0);

/// Power-of-two count of buffer registries. Each minting thread claims one; domain zero is shared
/// by teardown and by threads beyond the limit, so exhaustion degrades to one global registry.
const DOMAINS: usize = 1024;
const DOMAIN_SHIFT: u32 = 48;
const MAX_SEQUENCE: u64 = (1 << DOMAIN_SHIFT) - 1;

type Registry = Mutex<IntMap<u64, Storage>>;

static REGISTRIES: LazyLock<Box<[Registry]>> = LazyLock::new(|| (0..DOMAINS).map(|_| Registry::default()).collect());
static NEXT_DOMAIN: AtomicU64 = AtomicU64::new(1);
static FREE_DOMAINS: LazyLock<Mutex<Vec<u64>>> = LazyLock::new(Mutex::default);

/// Returns an empty domain for reuse. Sequences stay globally unique, so handles never recycle.
struct Domain(u64);

impl Drop for Domain {
  fn drop(&mut self) {
    if self.0 != 0 && lock(&REGISTRIES[self.0 as usize]).is_empty() {
      lock(&FREE_DOMAINS).push(self.0);
    }
  }
}

thread_local! {
  static DOMAIN: Domain = Domain(lock(&FREE_DOMAINS).pop().unwrap_or_else(|| {
    NEXT_DOMAIN
      .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
        (next < DOMAINS as u64).then_some(next + 1)
      })
      .unwrap_or(0)
  }));
  static BUDGET: RefCell<Option<(u64, Budget)>> = const { RefCell::new(None) };
  // Separate from BUDGET so alternating owner and workload lookups do not evict each other.
  static WORKLOAD: RefCell<Option<(u64, Budget)>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Default)]
struct Operation {
  socket: u64,
  buffer: u64,
  http: bool,
}

impl Operation {
  fn raw(socket: u64, buffer: u64) -> Self {
    Self {
      socket,
      buffer,
      http: false,
    }
  }

  fn http(socket: u64) -> Self {
    Self {
      socket,
      buffer: 0,
      http: true,
    }
  }
}

/// An admitted workload: the owner id that names it and the budget whose closure stops its work.
#[derive(Clone)]
struct Workload {
  id: u64,
  budget: Budget,
}

impl Workload {
  /// Resolve an open workload for new work; unknown, released, and closed workloads are rejected.
  fn admit(id: u64) -> Option<Self> {
    memoized(&WORKLOAD, id)
      .filter(Budget::admit_work)
      .map(|budget| Self { id, budget })
  }

  fn closed(&self) -> bool {
    self.budget.is_closed()
  }

  /// Whether `id` names this workload and it still admits work.
  fn admits(&self, id: u64) -> bool {
    self.id == id && !self.closed()
  }
}

struct DriverState {
  driver: Driver,
  sockets: IntMap<u64, crate::driver::Connection>,
  /// Owning workload of each socket; a socket without an entry admits no further work.
  workloads: IntMap<u64, Workload>,
  /// Closure epoch this driver last swept.
  closures: u64,
  /// Workload of the last callback poll, so steady-state polls skip the owner registry.
  callback_workload: Option<Workload>,
  operations: IntMap<u64, Operation>,
  accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
  limit: usize,
  http: http::HttpTables,
  serving: Option<serving::Shard>,
  callback: socket::PollScratch,
  raw_poll: socket::PollScratch,
}

struct Drivers(IntMap<u64, DriverState>);

impl Drop for Drivers {
  fn drop(&mut self) {
    let mut wakers = lock(&WAKERS);
    for id in self.0.keys() {
      wakers.remove(id);
    }
  }
}

thread_local! {
  static DRIVERS: RefCell<Drivers> = RefCell::new(Drivers(IntMap::default()));
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
  value.lock().unwrap_or_else(|error| error.into_inner())
}

/// Registry owning a handle. A forged domain resolves to a table that cannot contain the handle,
/// so only genuine identities are ever found. At most one registry is locked at a time, and a
/// registry lock is only ever taken inside a driver or session borrow, never the reverse.
fn registry(handle: u64) -> &'static Registry {
  &REGISTRIES[(handle >> DOMAIN_SHIFT) as usize & (DOMAINS - 1)]
}

/// Registry receiving handles minted on this thread.
fn local() -> &'static Registry {
  &REGISTRIES[DOMAIN.try_with(|domain| domain.0).unwrap_or(0) as usize]
}

fn identity() -> u64 {
  let sequence = NEXT
    .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
      id.checked_add(1).filter(|next| *next <= MAX_SEQUENCE)
    })
    .unwrap_or(0);
  if sequence == 0 {
    return 0;
  }
  sequence | (DOMAIN.try_with(|domain| domain.0).unwrap_or(0) << DOMAIN_SHIFT)
}

/// Whether `socket` belongs to the open workload `workload`.
fn owned(state: &DriverState, socket: u64, workload: u64) -> bool {
  state.workloads.get(&socket).is_some_and(|owner| owner.admits(workload))
}

/// Stop a workload's admission and cancel its operations on this thread's drivers now, and on
/// other threads' drivers at their next poll. Owners that never named work only stop allocating.
fn close_workload(budget: &Budget) {
  budget.close();
  if !budget.is_workload() {
    return;
  }
  CLOSURES.fetch_add(1, Ordering::AcqRel);
  let _ = DRIVERS.try_with(|drivers| {
    if let Ok(mut drivers) = drivers.try_borrow_mut() {
      for state in drivers.0.values_mut() {
        sweep(state);
      }
    }
  });
  let wakers: Vec<Waker> = lock(&WAKERS).values().cloned().collect();
  for waker in wakers {
    waker.wake();
  }
}

/// Cancel work owned by closed workloads. HTTP sockets close through their retirement path; other
/// sockets lose their workload and keep their handle until the consumer closes it.
fn sweep(state: &mut DriverState) {
  let epoch = CLOSURES.load(Ordering::Acquire);
  if state.closures == epoch {
    return;
  }
  state.closures = epoch;
  let closed: Vec<u64> = state
    .workloads
    .iter()
    .filter(|(_, workload)| workload.closed())
    .map(|(socket, _)| *socket)
    .collect();
  let mut events = std::collections::VecDeque::new();
  for socket in closed {
    if http::is_http(state, socket) {
      http::close_socket(state, socket, &mut events);
      continue;
    }
    state.workloads.remove(&socket);
    for (operation, pending) in &state.operations {
      if pending.socket == socket {
        state.driver.cancel(*operation);
      }
    }
  }
  state.http.overflow.extend(events);
}

/// Resolve an owner budget, memoizing the last one used on this thread.
///
/// Owner identities never recycle and `close` propagates through every clone, so a memoized budget
/// cannot outlive its owner's admission policy.
fn budget(owner: u64) -> Option<Budget> {
  memoized(&BUDGET, owner)
}

fn memoized(memo: &'static std::thread::LocalKey<RefCell<Option<(u64, Budget)>>>, owner: u64) -> Option<Budget> {
  let cached = memo.try_with(|cache| {
    let mut cache = cache.borrow_mut();
    if let Some((id, budget)) = cache.as_ref()
      && *id == owner
    {
      return Some(budget.clone());
    }
    let budget = lock(&OWNERS).get(&owner).cloned()?;
    *cache = Some((owner, budget.clone()));
    Some(budget)
  });
  match cached {
    Ok(budget) => budget,
    Err(_) => lock(&OWNERS).get(&owner).cloned(),
  }
}

/// ABI major version; callers must reject mismatched libraries before submitting work.
pub fn elide_transport_abi_version() -> u32 {
  3
}

/// Create a native allocation budget, which is also a workload token; zero indicates invalid input
/// or identity exhaustion.
pub fn elide_transport_owner_new(limit: u64) -> u64 {
  let Ok(limit) = usize::try_from(limit) else {
    return 0;
  };
  if limit == 0 || limit > isize::MAX as usize {
    return 0;
  }
  let id = identity();
  if id != 0 {
    lock(&OWNERS).insert(id, Budget::new(limit));
  }
  id
}

/// Return retained capacity, or u64::MAX for an invalid owner.
pub fn elide_transport_owner_used(owner: u64) -> u64 {
  lock(&OWNERS)
    .get(&owner)
    .map_or(u64::MAX, |budget| budget.used() as u64)
}

/// Close the owner's workload and forget its handle. Existing buffers retain their budget until
/// release.
pub fn elide_transport_owner_release(owner: u64) -> i32 {
  let budget = lock(&OWNERS).remove(&owner);
  if let Some(budget) = budget {
    close_workload(&budget);
    0
  } else {
    INVALID
  }
}

/// Stop a workload's allocations and new work, and cancel its operations. Other workloads sharing
/// its drivers are untouched. The handle stays valid for `owner_used` until `owner_release`.
pub fn elide_transport_workload_close(workload: u64) -> i32 {
  let budget = lock(&OWNERS).get(&workload).cloned();
  if let Some(budget) = budget {
    close_workload(&budget);
    0
  } else {
    INVALID
  }
}

/// Allocate zero-initialized foreign-accessible storage; zero indicates failure.
pub fn elide_transport_buffer_new(owner: u64, capacity: u64) -> u64 {
  let Ok(capacity) = usize::try_from(capacity) else {
    return 0;
  };
  let Some(budget) = budget(owner) else {
    return 0;
  };
  let Ok(mut buffer) = Buffer::new(capacity, budget) else {
    return 0;
  };
  buffer.ensure_init();
  let id = identity();
  if id != 0 {
    lock(registry(id)).insert(id, Storage::Mutable(buffer));
  }
  id
}

/// Borrow a descriptor until release or ownership transition.
///
/// # Safety
/// `output` must be writable and aligned. The caller must retain the handle, serialize mutable
/// access, and stop using all mutable addresses before freezing or submitting storage.
pub unsafe fn elide_transport_buffer_view(handle: u64, output: *mut BufferView) -> i32 {
  if output.is_null() || !output.is_aligned() {
    return INVALID;
  }
  let mut buffers = lock(registry(handle));
  let Some(storage) = buffers.get_mut(&handle) else {
    return INVALID;
  };
  let view = match storage {
    Storage::Mutable(buffer) => {
      let capacity = buffer.buf_capacity() as u64;
      let length = IoBuf::buf_len(&*buffer) as u64;
      // Minted last: `buf_capacity` reborrows the region through `&mut`, which would invalidate
      // an address handed to the caller before it.
      let address = buffer.buf_mut_ptr().cast();
      BufferView {
        address,
        capacity,
        length,
        flags: 0,
      }
    }
    Storage::Frozen(buffer) => BufferView {
      address: buffer.as_ref().as_ptr().cast_mut().cast(),
      capacity: buffer.as_ref().len() as u64,
      length: buffer.as_ref().len() as u64,
      flags: READ_ONLY,
    },
  };
  // SAFETY: The caller supplies a non-null, aligned writable BufferView output.
  unsafe { output.write(view) };
  0
}

/// Transfer foreign mutation rights to immutable native ownership.
///
/// # Safety
/// All foreign writers and mutable views must be quiescent and abandoned before this call.
pub unsafe fn elide_transport_buffer_freeze(handle: u64, length: u64) -> i32 {
  let Ok(length) = usize::try_from(length) else {
    return INVALID;
  };
  let mut buffers = lock(registry(handle));
  let Some(Storage::Mutable(buffer)) = buffers.get_mut(&handle) else {
    return INVALID;
  };
  if length > buffer.buf_capacity() {
    return INVALID;
  }
  // All foreign-accessible capacity was initialized on allocation.
  // SAFETY: Allocation initialized all foreign-accessible capacity; length is checked above.
  unsafe { buffer.set_len(length) };
  let Some(Storage::Mutable(buffer)) = buffers.remove(&handle) else {
    return INVALID;
  };
  buffers.insert(handle, Storage::Frozen(buffer.freeze()));
  0
}

/// Retain an immutable subregion; zero indicates invalid input or exhausted identities.
pub fn elide_transport_buffer_slice(handle: u64, offset: u64, length: u64) -> u64 {
  let (Ok(offset), Ok(length)) = (usize::try_from(offset), usize::try_from(length)) else {
    return 0;
  };
  let Some(end) = offset.checked_add(length) else {
    return 0;
  };
  let slice = {
    let buffers = lock(registry(handle));
    let Some(Storage::Frozen(buffer)) = buffers.get(&handle) else {
      return 0;
    };
    let Ok(slice) = buffer.slice(offset..end) else {
      return 0;
    };
    slice
  };
  let id = identity();
  if id != 0 {
    lock(registry(id)).insert(id, Storage::Frozen(slice));
  }
  id
}

/// Release one handle. Retained views and in-flight operations remain valid.
pub fn elide_transport_buffer_release(handle: u64) -> i32 {
  let storage = lock(registry(handle)).remove(&handle);
  if storage.is_some() { 0 } else { INVALID }
}

/// Create an owner-thread driver; zero indicates a closed workload, unsupported selection, or
/// initialization failure.
pub fn elide_transport_driver_new(workload: u64, backend: u32, limit: u32) -> u64 {
  if Workload::admit(workload).is_none() {
    return 0;
  }
  let backend = match backend {
    0 => Backend::Auto,
    1 => Backend::Polling,
    2 => Backend::IoUring,
    3 => Backend::Iocp,
    _ => return 0,
  };
  let driver = match Driver::new(backend, limit as usize) {
    Ok(driver) => driver,
    Err(error) => {
      // Surface the native failure (EMFILE, ENOMEM, unsupported backend) through last_error.
      set_last_error(&error);
      eprintln!("native transport initialization failed: {error}");
      return 0;
    }
  };
  let id = identity();
  if id != 0 {
    lock(&WAKERS).insert(id, driver.waker());
    DRIVERS.with(|drivers| {
      drivers.borrow_mut().0.insert(
        id,
        DriverState {
          driver,
          sockets: IntMap::with_capacity_and_hasher((limit as usize).min(4096), Default::default()),
          workloads: IntMap::default(),
          closures: CLOSURES.load(Ordering::Acquire),
          callback_workload: None,
          operations: IntMap::with_capacity_and_hasher((limit as usize).min(4096), Default::default()),
          accepted: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
          limit: limit as usize,
          http: http::HttpTables::default(),
          serving: None,
          callback: socket::PollScratch::default(),
          raw_poll: socket::PollScratch::default(),
        },
      )
    });
  }
  id
}

/// Return the actual backend on the owner thread, or INVALID.
pub fn elide_transport_driver_backend(handle: u64) -> i32 {
  DRIVERS.with(|drivers| {
    drivers
      .borrow()
      .0
      .get(&handle)
      .map_or(INVALID, |state| state.driver.backend() as i32)
  })
}

/// Write why AUTO fell back from io_uring (UTF-8, truncated to the capacity of mutable buffer
/// `output`) and return its length; zero when the requested backend runs. Owner thread only.
pub fn elide_transport_driver_fallback(handle: u64, output: u64) -> i32 {
  DRIVERS.with(|drivers| {
    let drivers = drivers.borrow();
    let Some(state) = drivers.0.get(&handle) else {
      return INVALID;
    };
    let Some(error) = state.driver.fallback() else {
      return 0;
    };
    let reason = error.to_string();
    let mut buffers = lock(registry(output));
    let Some(Storage::Mutable(buffer)) = buffers.get_mut(&output) else {
      return INVALID;
    };
    let mut end = reason.len().min(buffer.buf_capacity());
    while !reason.is_char_boundary(end) {
      end -= 1;
    }
    match buffer.write(0, &reason.as_bytes()[..end]) {
      Ok(()) => end as i32,
      Err(_) => INVALID,
    }
  })
}

/// Wake a driver from any thread. Unknown or released handles are rejected.
pub fn elide_transport_driver_wake(handle: u64) -> i32 {
  let wake = lock(&WAKERS).get(&handle).cloned();
  if let Some(wake) = wake {
    wake.wake();
    0
  } else {
    INVALID
  }
}

/// Release a driver on its owner thread.
pub fn elide_transport_driver_release(handle: u64) -> i32 {
  let status = DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(driver) = drivers.0.get_mut(&handle) else {
      return INVALID;
    };
    match driver.driver.try_shutdown(std::time::Duration::ZERO) {
      Ok(true) => 0,
      Ok(false) => BUSY,
      Err(_) => INVALID,
    }
  });
  if status != 0 {
    return status;
  }
  let driver = DRIVERS.with(|drivers| drivers.borrow_mut().0.remove(&handle));
  if let Some(mut driver) = driver {
    http::retire(&mut driver);
    lock(&WAKERS).remove(&handle);
    drop(driver);
    0
  } else {
    INVALID
  }
}
