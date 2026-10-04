/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Native startup coordination; live sockets and HTTP state never leave their owning shard.

use super::*;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8};

const STARTING: u8 = 0;
const READY: u8 = 1;
const CLOSED: u8 = 2;
/// A shard-local listener closed; `socket` identifies its local listener handle.
pub const EVENT_LISTENER_CLOSED: u32 = 11;
/// Application phase changed; `value` is 1 (ready) or 2 (closed).
pub const EVENT_SERVING_PHASE: u32 = 12;

struct Listener {
  requested: SocketAddr,
  bound: SocketAddr,
  signature: Vec<u8>,
  closed: AtomicBool,
}

#[derive(Default)]
struct Replica {
  waker: Option<Waker>,
  registrations: Option<usize>,
}

struct Startup {
  replicas: Vec<Replica>,
  listeners: Vec<Arc<Listener>>,
}

struct Application {
  count: usize,
  workers: usize,
  placements: Vec<Vec<usize>>,
  phase: AtomicU8,
  startup: Mutex<Startup>,
}

fn transport_capacity(contexts: usize, budget: usize) -> usize {
  contexts.saturating_mul(2).saturating_sub(2).max(4).min(budget)
}

impl Application {
  fn wake(&self) {
    let startup = lock(&self.startup);
    for replica in &startup.replicas {
      if let Some(waker) = &replica.waker {
        waker.wake_by_ref();
      }
    }
  }

  fn close(&self) {
    self.phase.store(CLOSED, Ordering::Release);
    self.wake();
  }
}

struct LocalListener {
  socket: u64,
  listener: Arc<Listener>,
  accepting: bool,
  closed: bool,
}

pub(super) struct Shard {
  pub(super) handoff: bool,
  application: Arc<Application>,
  index: usize,
  ready: bool,
  observed_phase: u8,
  listeners: Vec<LocalListener>,
  connections: IntMap<u64, u64>,
  placement: placement::Placement,
}

impl Drop for Shard {
  fn drop(&mut self) {
    self.application.close();
  }
}

static APPLICATIONS: LazyLock<Mutex<IntMap<u64, Arc<Application>>>> = LazyLock::new(Mutex::default);

thread_local! {
  static CONTEXT_PLACEMENT: RefCell<Option<placement::Placement>> = const { RefCell::new(None) };
}

/// Available physical cores in the caller's allowed affinity mask; nonpositive on failure.
pub fn elide_transport_serving_available_cores() -> i32 {
  #[cfg(target_os = "linux")]
  let count = placement::topology().map(|cores| cores.len());
  #[cfg(not(target_os = "linux"))]
  let count: io::Result<usize> = Ok(1);
  match count.and_then(|count| i32::try_from(count).map_err(|_| io::ErrorKind::InvalidData.into())) {
    Ok(count) if count > 0 => count,
    result => {
      socket::set_last_error(&result.err().unwrap_or_else(|| io::ErrorKind::NotFound.into()));
      INVALID
    }
  }
}

/// Pin the guest owner to its core's second allowed sibling, or its sole CPU.
/// Start the transport thread before entering, so it inherits the full allowed mask.
pub fn elide_transport_serving_context_enter(application: u64, replica: u32) -> i32 {
  let result = CONTEXT_PLACEMENT.with(|slot| {
    let mut slot = slot.borrow_mut();
    if slot.is_some() {
      return Err(io::ErrorKind::AlreadyExists.into());
    }
    let application = lock(&APPLICATIONS)
      .get(&application)
      .cloned()
      .ok_or(io::ErrorKind::InvalidInput)?;
    if replica as usize >= application.count || application.phase.load(Ordering::Acquire) == CLOSED {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let core = &application.placements[replica as usize];
    let cpu = core.get(1).or_else(|| core.first()).copied();
    *slot = Some(placement::Placement::pin(cpu)?);
    Ok(())
  });
  match result {
    Ok(()) => 0,
    Err(error) => {
      socket::set_last_error(&error);
      INVALID
    }
  }
}

/// Restore the calling guest owner's original CPU mask; nonzero on failure.
pub fn elide_transport_serving_context_leave() -> i32 {
  let result = CONTEXT_PLACEMENT.with(|slot| {
    let mut slot = slot.borrow_mut();
    slot.as_mut().ok_or(io::ErrorKind::InvalidInput)?.restore()?;
    *slot = None;
    Ok(())
  });
  match result {
    Ok(()) => 0,
    Err(error) => {
      socket::set_last_error(&error);
      INVALID
    }
  }
}

/// Capture a pinned parent's original CPU mask for one child; zero means no restoration needed.
/// This hook only reads TLS and increments a reference count; it never allocates or locks.
pub fn elide_transport_serving_helper_prepare() -> u64 {
  placement::helper_prepare()
}

/// Release an unconsumed child token without changing the caller's affinity.
/// Reclamation is deferred; this hook never allocates, frees, locks, or issues syscalls.
/// # Safety
/// Pass zero or one owned token from helper_prepare. Consume each owned token exactly once.
pub unsafe fn elide_transport_serving_helper_release(token: u64) -> i32 {
  unsafe { placement::helper_release(token) };
  0
}

/// Consume the parent's captured mask on a new helper; never unpin an existing serving owner.
/// Returns zero on success, or INVALID if the OS rejects placement.
/// # Safety
/// Pass zero or one owned token from helper_prepare. The token is consumed even on failure.
pub unsafe fn elide_transport_serving_helper_start(token: u64) -> i32 {
  match unsafe { placement::helper_consume(token) } {
    Ok(()) => 0,
    Err(error) => {
      socket::set_last_error(&error);
      INVALID
    }
  }
}

/// Allocate startup coordination for a fixed number of independent serving shards. Zero fails.
pub fn elide_transport_serving_new(count: u32) -> u64 {
  new_application(count, false)
}

/// Resolve a fixed transport allocation for split serving within the allowed physical CPU set.
pub fn elide_transport_serving_split_new(count: u32) -> u64 {
  new_application(count, true)
}

/// Number of transport owners assigned exclusively to this context; nonpositive on failure.
pub fn elide_transport_serving_workers(application: u64, context: u32) -> i32 {
  let applications = lock(&APPLICATIONS);
  let Some(application) = applications.get(&application) else {
    return INVALID;
  };
  if context as usize >= application.count || application.phase.load(Ordering::Acquire) == CLOSED {
    return INVALID;
  }
  ((application.workers - 1 - context as usize) / application.count + 1) as i32
}

/// Create a context's indexed transport owner. Assignment and placement are resolved natively.
pub fn elide_transport_serving_worker_driver(
  workload: u64,
  application: u64,
  context: u32,
  worker: u32,
  backend: u32,
  limit: u32,
) -> u64 {
  let index = {
    let applications = lock(&APPLICATIONS);
    let Some(application) = applications.get(&application) else {
      return 0;
    };
    if context as usize >= application.count {
      return 0;
    }
    let Some(index) = (worker as usize)
      .checked_mul(application.count)
      .and_then(|offset| offset.checked_add(context as usize))
    else {
      return 0;
    };
    if index >= application.workers {
      return 0;
    }
    index as u32
  };
  elide_transport_serving_driver(workload, application, index, backend, limit, 1)
}

fn new_application(count: u32, split: bool) -> u64 {
  if count == 0 || (count > 1 && !cfg!(target_os = "linux")) {
    socket::set_last_error(&io::ErrorKind::Unsupported.into());
    return 0;
  }
  let mut replicas = Vec::new();
  let cores = match placement::topology() {
    Ok(cores) => cores,
    Err(error) => {
      socket::set_last_error(&error);
      return 0;
    }
  };
  let physical = cores.len();
  let siblings = !split && count as usize > physical;
  let capacity = if siblings {
    cores.iter().map(Vec::len).sum()
  } else {
    physical
  };
  if count as usize > capacity {
    socket::set_last_error(&io::Error::new(
      io::ErrorKind::InvalidInput,
      if siblings {
        "serving contexts exceed available logical CPUs"
      } else {
        "serving contexts exceed available physical cores"
      },
    ));
    return 0;
  }
  let workers = if split && cfg!(target_os = "linux") {
    transport_capacity(count as usize, cores.len())
  } else {
    count as usize
  };
  let placements = if siblings {
    // Keep the physical-core order, then fill each remaining allowed sibling layer.
    let cores = placement::serving_cores(cores, physical, physical);
    placement::sibling_placements(cores)
  } else {
    placement::serving_cores(cores, count as usize, workers)
  };
  if replicas.try_reserve_exact(workers).is_err() {
    return 0;
  }
  replicas.resize_with(workers, Replica::default);
  let id = identity();
  if id != 0 {
    lock(&APPLICATIONS).insert(
      id,
      Arc::new(Application {
        count: count as usize,
        workers,
        placements,
        phase: AtomicU8::new(STARTING),
        startup: Mutex::new(Startup {
          replicas,
          listeners: Vec::new(),
        }),
      }),
    );
  }
  id
}

/// Create this replica's reactor on the calling OS thread. An endpoint may be claimed only once.
/// Placement bit 0 pins the owner; bit 1 transfers accepted sockets to separate transport owners.
pub fn elide_transport_serving_driver(
  workload: u64,
  application: u64,
  index: u32,
  backend: u32,
  limit: u32,
  affinity: u32,
) -> u64 {
  if affinity > 3 {
    return 0;
  }
  let Some(application) = lock(&APPLICATIONS).get(&application).cloned() else {
    return 0;
  };
  if DRIVERS.with(|drivers| drivers.borrow().0.values().any(|state| state.serving.is_some())) {
    return 0;
  }
  let mut startup = lock(&application.startup);
  if application.phase.load(Ordering::Acquire) != STARTING {
    return 0;
  }
  let Some(replica) = startup.replicas.get_mut(index as usize) else {
    return 0;
  };
  if replica.waker.is_some() {
    return 0;
  }
  let cpu = if affinity & 1 == 0 {
    None
  } else {
    application.placements[index as usize].first().copied()
  };
  let placement = match placement::Placement::pin(cpu) {
    Ok(placement) => placement,
    Err(error) => {
      socket::set_last_error(&error);
      drop(startup);
      application.close();
      return 0;
    }
  };
  let driver = elide_transport_driver_new(workload, backend, limit);
  if driver == 0 {
    drop(startup);
    application.close();
    return 0;
  }
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let state = drivers.0.get_mut(&driver).unwrap();
    replica.waker = Some(state.driver.waker());
    state.serving = Some(Shard {
      handoff: affinity & 2 != 0,
      application: application.clone(),
      index: index as usize,
      ready: false,
      observed_phase: STARTING,
      listeners: Vec::new(),
      connections: IntMap::default(),
      placement,
    });
  });
  driver
}

/// Bind a shard-local listener of `workload`, whose closure closes the listener and its connections.
/// Startup registrations match by ordinal and opaque configuration. Endpoints and signatures are
/// frozen buffers; zero fails and cancels the application.
pub fn elide_transport_serving_listen(workload: u64, driver: u64, endpoint: u64, signature: u64, backlog: i32) -> u64 {
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  let Some(requested) = socket::address(endpoint) else {
    return 0;
  };
  let signature = {
    let buffers = lock(registry(signature));
    let Some(Storage::Frozen(bytes)) = buffers.get(&signature) else {
      return 0;
    };
    bytes.as_ref().to_vec()
  };
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    let Some(shard) = state.serving.as_ref() else {
      return 0;
    };
    let application = shard.application.clone();
    let result = (|| -> io::Result<(u64, LocalListener)> {
      if application.phase.load(Ordering::Acquire) == CLOSED {
        return Err(io::ErrorKind::BrokenPipe.into());
      }
      let mut startup = lock(&application.startup);
      let ordinal = shard.listeners.len();
      let grouped = !shard.ready;
      let existing = if grouped {
        startup.listeners.get(ordinal).cloned()
      } else {
        None
      };
      if grouped
        && startup
          .replicas
          .iter()
          .any(|replica| replica.registrations.is_some_and(|count| ordinal >= count))
      {
        return Err(io::ErrorKind::InvalidInput.into());
      }
      if let Some(listener) = &existing
        && (listener.requested != requested
          || listener.signature != signature
          || listener.closed.load(Ordering::Acquire))
      {
        return Err(io::ErrorKind::InvalidInput.into());
      }
      let address = existing.as_ref().map_or(requested, |listener| listener.bound);
      let connection = if grouped && application.workers > 1 {
        state.driver.listen_sharded(address, backlog)?
      } else {
        state.driver.listen(address, backlog)?
      };
      let bound = connection.local_address()?;
      let listener = existing.unwrap_or_else(|| {
        Arc::new(Listener {
          requested,
          bound,
          signature,
          closed: AtomicBool::new(false),
        })
      });
      if grouped && ordinal == startup.listeners.len() {
        startup.listeners.push(listener.clone());
      }
      let socket = identity();
      if socket == 0 {
        return Err(io::ErrorKind::OutOfMemory.into());
      }
      state.sockets.insert(socket, connection);
      state.workloads.insert(socket, workload);
      Ok((
        socket,
        LocalListener {
          socket,
          listener,
          accepting: false,
          closed: false,
        },
      ))
    })();
    match result {
      Ok((socket, listener)) => {
        state.serving.as_mut().unwrap().listeners.push(listener);
        socket
      }
      Err(error) => {
        socket::set_last_error(&error);
        application.close();
        0
      }
    }
  })
}

/// Mark this replica's completed entry evaluation. Acceptance begins only after every replica.
pub fn elide_transport_serving_ready(driver: u64) -> i32 {
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(shard) = drivers.0.get_mut(&driver).and_then(|state| state.serving.as_mut()) else {
      return INVALID;
    };
    if shard.application.phase.load(Ordering::Acquire) == CLOSED {
      return INVALID;
    }
    if shard.ready {
      return 0;
    }
    let mut startup = lock(&shard.application.startup);
    if shard.listeners.len() != startup.listeners.len() {
      drop(startup);
      shard.application.close();
      return INVALID;
    }
    shard.ready = true;
    startup.replicas[shard.index].registrations = Some(shard.listeners.len());
    let ready = startup.replicas.iter().all(|replica| replica.registrations.is_some());
    if ready
      && shard
        .application
        .phase
        .compare_exchange(STARTING, READY, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
      return INVALID;
    }
    drop(startup);
    if ready {
      shard.application.wake();
    }
    0
  })
}

/// Assigned logical CPU on the owner thread; -1 when placement is unavailable or the handle is invalid.
pub fn elide_transport_serving_cpu(driver: u64) -> i32 {
  DRIVERS.with(|drivers| {
    drivers
      .borrow()
      .0
      .get(&driver)
      .and_then(|state| state.serving.as_ref())
      .and_then(|shard| shard.placement.cpu)
      .map_or(-1, |cpu| cpu as i32)
  })
}

/// Close a logical listener across its shards; teardown executes on each socket's owning thread.
pub fn elide_transport_serving_listener_close(driver: u64, listener: u64) -> i32 {
  DRIVERS.with(|drivers| {
    let drivers = drivers.borrow();
    let Some(shard) = drivers.0.get(&driver).and_then(|state| state.serving.as_ref()) else {
      return INVALID;
    };
    let Some(local) = shard.listeners.iter().find(|local| local.socket == listener) else {
      return INVALID;
    };
    local.listener.closed.store(true, Ordering::Release);
    shard.application.wake();
    0
  })
}

/// Stop all shards and release the coordinator handle. Live reactors retain it until retirement.
pub fn elide_transport_serving_close(application: u64) -> i32 {
  let Some(application) = lock(&APPLICATIONS).remove(&application) else {
    return INVALID;
  };
  application.close();
  0
}

pub(super) fn accepted(state: &mut DriverState, listener: u64) -> bool {
  if let Some(shard) = &mut state.serving {
    if let Some(local) = shard.listeners.iter_mut().find(|local| local.socket == listener) {
      local.accepting = false;
      return !local.closed
        && !local.listener.closed.load(Ordering::Acquire)
        && shard.application.phase.load(Ordering::Acquire) == READY;
    }
    return false;
  }
  true
}

pub(super) fn connected(state: &mut DriverState, listener: u64, socket: u64) {
  if let Some(shard) = &mut state.serving {
    shard.connections.insert(socket, listener);
  }
}

pub(super) fn disconnected(state: &mut DriverState, socket: u64) {
  if let Some(shard) = &mut state.serving {
    shard.connections.remove(&socket);
  }
}

pub(super) fn prepare(state: &mut DriverState) {
  let Some(mut shard) = state.serving.take() else {
    return;
  };
  let phase = shard.application.phase.load(Ordering::Acquire);
  for local in &mut shard.listeners {
    if local.closed {
      continue;
    }
    let owned = state
      .workloads
      .get(&local.socket)
      .is_some_and(|workload| !workload.closed());
    if phase == CLOSED || !owned || local.listener.closed.load(Ordering::Acquire) {
      local.closed = true;
      state.http.overflow.retain(|event| {
        if event.kind != 2 || event.socket != local.socket {
          return true;
        }
        if shard.handoff {
          socket::elide_transport_socket_discard(event.value);
        }
        false
      });
      state.workloads.remove(&local.socket);
      if let Some(connection) = state.sockets.remove(&local.socket) {
        let _ = connection.shutdown(std::net::Shutdown::Both);
        for (operation, pending) in &state.operations {
          if pending.socket == local.socket {
            state.driver.cancel(*operation);
          }
        }
      }
      let sockets: Vec<_> = shard
        .connections
        .iter()
        .filter(|(_, listener)| **listener == local.socket)
        .map(|(socket, _)| *socket)
        .collect();
      let mut events = std::collections::VecDeque::new();
      for socket in sockets {
        shard.connections.remove(&socket);
        http::close_socket(state, socket, &mut events);
      }
      state.http.overflow.extend(events);
      state.http.overflow.push_back(socket::NativeEvent {
        operation: 0,
        socket: local.socket,
        value: 0,
        result: 0,
        kind: EVENT_LISTENER_CLOSED,
        reserved: 0,
      });
    } else if phase == READY
      && !local.accepting
      && let Some(connection) = state.sockets.get(&local.socket)
    {
      match state.driver.accept(connection) {
        Ok(operation) => {
          state.operations.insert(operation, Operation::raw(local.socket, 0));
          local.accepting = true;
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Err(_) => shard.application.close(),
      }
    }
  }
  if phase != shard.observed_phase {
    shard.observed_phase = phase;
    state.http.overflow.push_back(socket::NativeEvent {
      operation: 0,
      socket: 0,
      value: phase as u64,
      result: 0,
      kind: EVENT_SERVING_PHASE,
      reserved: 0,
    });
  }
  state.serving = Some(shard);
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;
  use std::net::TcpStream;
  use std::time::{Duration, Instant};

  #[cfg(target_os = "linux")]
  fn workload() -> u64 {
    elide_transport_owner_new(1)
  }

  #[test]
  fn transport_capacity_has_a_global_floor_and_respects_physical_budget() {
    for (contexts, budget, expected) in [
      (1, 12, 4),
      (2, 12, 4),
      (3, 12, 4),
      (4, 12, 6),
      (8, 12, 12),
      (10, 12, 12),
      (12, 12, 12),
      (1, 2, 2),
      (1, 1, 1),
    ] {
      assert_eq!(transport_capacity(contexts, budget), expected);
    }
  }

  #[cfg(target_os = "linux")]
  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn split_workers_have_distinct_owners_and_one_startup_barrier() {
    assert_split_owners(1, &[(0, 0), (0, 1), (0, 2), (0, 3)]);
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn uneven_transport_assignment_keeps_secondary_owners_context_local() {
    assert_split_owners(3, &[(0, 0), (1, 0), (2, 0), (0, 1)]);
  }

  #[cfg(target_os = "linux")]
  fn assert_split_owners(contexts: u32, assignments: &[(u32, u32)]) {
    let cores = placement::topology().unwrap();
    if cores.len() < contexts as usize {
      return;
    }
    let application = elide_transport_serving_split_new(contexts);
    assert_ne!(application, 0);
    let cores = lock(&APPLICATIONS).get(&application).unwrap().placements.clone();
    let count = 4.min(cores.len());
    let expected = if contexts == 1 {
      vec![count as i32]
    } else if count == 4 {
      vec![2, 1, 1]
    } else {
      vec![1, 1, 1]
    };
    for (context, workers) in expected.into_iter().enumerate() {
      assert_eq!(elide_transport_serving_workers(application, context as u32), workers);
    }
    assert_eq!(elide_transport_serving_workers(application, contexts), INVALID);
    assert_eq!(
      elide_transport_serving_worker_driver(workload(), application, 0, count as u32, 1, 16),
      0
    );
    let finish = Arc::new(std::sync::Barrier::new(count + 1));
    let (send, recv) = std::sync::mpsc::channel();
    let threads: Vec<_> = (0..count)
      .map(|index| {
        let finish = finish.clone();
        let send = send.clone();
        let cpu = cores[index][0];
        let (context, worker) = assignments[index];
        std::thread::spawn(move || {
          let before = placement::allowed().unwrap();
          let driver = elide_transport_serving_worker_driver(workload(), application, context, worker, 1, 16);
          assert_ne!(driver, 0);
          assert_eq!(placement::allowed().unwrap(), vec![cpu]);
          assert_eq!(elide_transport_serving_ready(driver), 0);
          send.send(()).unwrap();
          finish.wait();
          assert_eq!(elide_transport_driver_release(driver), 0);
          assert_eq!(placement::allowed().unwrap(), before);
        })
      })
      .collect();
    for _ in 0..count {
      recv.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    assert_eq!(lock(&APPLICATIONS)[&application].phase.load(Ordering::Acquire), READY);
    finish.wait();
    for thread in threads {
      thread.join().unwrap();
    }
    assert_eq!(elide_transport_serving_close(application), 0);
  }

  #[test]
  fn context_placement_rejects_invalid_and_duplicate_entry() {
    assert_eq!(elide_transport_serving_helper_prepare(), 0);
    assert_eq!(unsafe { elide_transport_serving_helper_start(0) }, 0);
    assert_eq!(unsafe { elide_transport_serving_helper_release(0) }, 0);
    assert!(elide_transport_serving_available_cores() > 0);
    #[cfg(not(target_os = "linux"))]
    assert_eq!(elide_transport_serving_available_cores(), 1);
    let application = elide_transport_serving_new(1);
    assert_ne!(application, 0);
    assert_eq!(elide_transport_serving_context_enter(0, 0), INVALID);
    assert_eq!(elide_transport_serving_context_enter(application, 1), INVALID);
    assert_eq!(elide_transport_serving_context_enter(application, 0), 0);
    #[cfg(target_os = "linux")]
    {
      let token = elide_transport_serving_helper_prepare();
      assert_ne!(token, 0);
      assert_eq!(unsafe { elide_transport_serving_helper_release(token) }, 0);
      let token = elide_transport_serving_helper_prepare();
      assert_eq!(unsafe { elide_transport_serving_helper_start(token) }, 0);
    }
    assert_eq!(elide_transport_serving_context_enter(application, 0), INVALID);
    assert_eq!(elide_transport_serving_context_leave(), 0);
    assert_eq!(elide_transport_serving_context_leave(), INVALID);
    assert_eq!(elide_transport_serving_close(application), 0);
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn explicit_inline_owners_fill_physical_cores_before_allowed_siblings() {
    let allowed = placement::allowed().unwrap();
    let cores = placement::topology().unwrap();
    if allowed.len() == cores.len() {
      return;
    }
    let application = elide_transport_serving_new(allowed.len() as u32);
    assert_ne!(application, 0, "explicit owners may use allowed SMT siblings");
    let assigned: Vec<_> = lock(&APPLICATIONS)[&application]
      .placements
      .iter()
      .map(|slot| {
        assert_eq!(slot.len(), 1);
        slot[0]
      })
      .collect();
    let mut sorted = assigned.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, allowed, "each allowed CPU appears exactly once");
    for core in &cores {
      assert_eq!(
        assigned[..cores.len()].iter().filter(|cpu| core.contains(cpu)).count(),
        1,
        "fill every physical core before its siblings"
      );
    }
    assert_eq!(elide_transport_serving_new(allowed.len() as u32 + 1), 0);
    assert_eq!(elide_transport_serving_split_new(cores.len() as u32 + 1), 0);
    let index = assigned.len() - 1;
    let cpu = assigned[index];
    std::thread::spawn(move || {
      let before = placement::allowed().unwrap();
      let driver = elide_transport_serving_driver(workload(), application, index as u32, 1, 16, 1);
      assert_ne!(driver, 0);
      assert_eq!(placement::allowed().unwrap(), vec![cpu]);
      assert_eq!(elide_transport_driver_release(driver), 0);
      assert_eq!(placement::allowed().unwrap(), before);
    })
    .join()
    .unwrap();
    assert_eq!(elide_transport_serving_close(application), 0);
  }

  #[cfg(target_os = "linux")]
  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn paired_owners_use_allowed_siblings_and_restore_their_masks() {
    let original = placement::allowed().unwrap();
    let cores = placement::topology().unwrap();
    let application = elide_transport_serving_new(1);
    assert_ne!(application, 0);
    assert_eq!(elide_transport_serving_split_new(cores.len() as u32 + 1), 0);
    let transport_cpu = cores[0][0];
    let guest_cpu = *cores[0].get(1).unwrap_or(&transport_cpu);
    let (started, wait) = std::sync::mpsc::channel();
    let (finish, finished) = std::sync::mpsc::channel();
    let transport = std::thread::spawn(move || {
      let before = placement::allowed().unwrap();
      let driver = elide_transport_serving_driver(workload(), application, 0, 1, 16, 1);
      assert_ne!(driver, 0);
      assert_eq!(placement::allowed().unwrap(), vec![transport_cpu]);
      assert_eq!(
        unsafe { elide_transport_serving_helper_start(elide_transport_serving_helper_prepare()) },
        0
      );
      assert_eq!(placement::allowed().unwrap(), vec![transport_cpu]);
      started.send(()).unwrap();
      finished.recv().unwrap();
      assert_eq!(elide_transport_driver_release(driver), 0);
      assert_eq!(placement::allowed().unwrap(), before);
      before
    });
    wait.recv().unwrap();
    assert_eq!(elide_transport_serving_context_enter(application, 0), 0);
    assert_eq!(placement::allowed().unwrap(), vec![guest_cpu]);
    assert_eq!(
      unsafe { elide_transport_serving_helper_start(elide_transport_serving_helper_prepare()) },
      0
    );
    assert_eq!(placement::allowed().unwrap(), vec![guest_cpu]);
    assert_eq!(elide_transport_serving_context_enter(application, 0), INVALID);
    assert_eq!(placement::allowed().unwrap(), vec![guest_cpu]);
    assert_eq!(elide_transport_serving_context_leave(), 0);
    assert_eq!(placement::allowed().unwrap(), original);
    finish.send(()).unwrap();
    assert_eq!(transport.join().unwrap(), original);
    assert_eq!(elide_transport_serving_close(application), 0);
  }

  struct Fixture {
    application: u64,
    driver: u64,
    owner: u64,
    batch: u64,
    listener: u64,
    socket: u64,
    peer: TcpStream,
  }

  impl Fixture {
    fn new() -> Self {
      Self::with_placement(1)
    }

    fn with_placement(placement: u32) -> Self {
      let application = elide_transport_serving_new(1);
      let owner = elide_transport_owner_new(1024 * 1024);
      let driver = elide_transport_serving_driver(owner, application, 0, 0, 32, placement);
      assert_ne!(driver, 0);
      let batch = elide_transport_buffer_new(owner, 8 * 40);
      let endpoint = elide_transport_buffer_new(owner, 24);
      let mut view = BufferView::default();
      assert_eq!(unsafe { elide_transport_buffer_view(endpoint, &mut view) }, 0);
      let address = unsafe { std::slice::from_raw_parts_mut(view.address.cast::<u8>(), 24) };
      address.fill(0);
      address[..4].copy_from_slice(&[127, 0, 0, 1]);
      address[18..20].copy_from_slice(&4u16.to_ne_bytes());
      assert_eq!(unsafe { elide_transport_buffer_freeze(endpoint, 24) }, 0);
      let listener = elide_transport_serving_listen(owner, driver, endpoint, endpoint, 64);
      assert_ne!(listener, 0);
      elide_transport_buffer_release(endpoint);
      assert_eq!(elide_transport_socket_address(driver, listener, 0, batch), 0);
      assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
      let address = unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), 24) };
      let port = u16::from_ne_bytes(address[16..18].try_into().unwrap());
      let peer = TcpStream::connect(("127.0.0.1", port)).unwrap();
      assert_eq!(elide_transport_serving_ready(driver), 0);
      let deadline = Instant::now() + Duration::from_secs(3);
      let socket = loop {
        assert!(Instant::now() < deadline);
        let count = unsafe { elide_transport_driver_poll(driver, 10_000_000, batch, 8) };
        assert!(count >= 0);
        let events = unsafe { std::slice::from_raw_parts(view.address.cast::<NativeEvent>(), count as usize) };
        if let Some(event) = events.iter().find(|event| event.kind == 2) {
          break event.value;
        }
      };
      Self {
        application,
        driver,
        owner,
        batch,
        listener,
        socket,
        peer,
      }
    }

    fn close_and_poll(&self) -> Vec<u32> {
      assert_eq!(elide_transport_serving_listener_close(self.driver, self.listener), 0);
      let count = unsafe { elide_transport_driver_poll(self.driver, 0, self.batch, 8) };
      assert!(count > 0);
      let mut view = BufferView::default();
      assert_eq!(unsafe { elide_transport_buffer_view(self.batch, &mut view) }, 0);
      unsafe { std::slice::from_raw_parts(view.address.cast::<NativeEvent>(), count as usize) }
        .iter()
        .map(|event| event.kind)
        .collect()
    }
  }

  impl Drop for Fixture {
    fn drop(&mut self) {
      elide_transport_serving_close(self.application);
      while elide_transport_driver_release(self.driver) == BUSY {
        unsafe { elide_transport_driver_poll(self.driver, 1_000_000, self.batch, 8) };
      }
      elide_transport_buffer_release(self.batch);
      if !std::thread::panicking() {
        assert_eq!(elide_transport_owner_used(self.owner), 0);
      }
      elide_transport_owner_release(self.owner);
    }
  }

  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn listener_close_discards_late_http_receive_storage() {
    let mut fixture = Fixture::new();
    assert_eq!(
      elide_transport_socket_http(fixture.owner, fixture.driver, fixture.socket, fixture.owner, 4096),
      0
    );
    fixture
      .peer
      .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
      .unwrap();
    DRIVERS.with(|drivers| {
      let mut drivers = drivers.borrow_mut();
      let state = drivers.0.get_mut(&fixture.driver).unwrap();
      let deadline = Instant::now() + Duration::from_secs(3);
      loop {
        assert!(Instant::now() < deadline);
        let events = state.driver.poll(Duration::from_millis(10), 32).unwrap();
        if events.is_empty() {
          continue;
        }
        assert!(
          events
            .iter()
            .any(|event| matches!(event, crate::driver::Event::Received { result: Ok(size), .. } if *size > 0))
        );
        state.driver.defer_completed(events);
        break;
      }
    });
    assert!(
      !fixture.close_and_poll().contains(&3),
      "closed HTTP receive escaped as a raw buffer"
    );
  }

  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn listener_close_discards_an_accept_deferred_in_overflow() {
    let fixture = Fixture::new();
    DRIVERS.with(|drivers| {
      drivers
        .borrow_mut()
        .0
        .get_mut(&fixture.driver)
        .unwrap()
        .http
        .overflow
        .push_back(NativeEvent {
          operation: 0,
          socket: fixture.listener,
          value: fixture.socket,
          result: 0,
          kind: 2,
          reserved: 0,
        });
    });
    assert!(
      !fixture.close_and_poll().contains(&2),
      "closed accepted socket was published"
    );
  }

  #[test]
  #[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
  fn split_listener_close_releases_an_undelivered_handoff() {
    let fixture = Fixture::with_placement(2);
    DRIVERS.with(|drivers| {
      drivers
        .borrow_mut()
        .0
        .get_mut(&fixture.driver)
        .unwrap()
        .http
        .overflow
        .push_back(NativeEvent {
          operation: 0,
          socket: fixture.listener,
          value: fixture.socket,
          result: 0,
          kind: 2,
          reserved: 0,
        });
    });
    assert!(!fixture.close_and_poll().contains(&2));
    assert_eq!(
      elide_transport_socket_discard(fixture.socket),
      INVALID,
      "listener close left the unadopted socket alive"
    );
  }
}
