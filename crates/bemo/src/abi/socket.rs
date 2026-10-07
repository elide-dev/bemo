/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Socket operations share the buffer and handle ownership domain of the base ABI.

use socket2::Socket;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV6};
use std::time::Duration;

use super::*;
use crate::driver::Event;

thread_local! { static LAST_ERROR: std::cell::Cell<i32> = const { std::cell::Cell::new(0) }; }

/// Last synchronous listen/connect/driver error on this thread; reading clears it.
pub fn elide_transport_last_error() -> i32 {
  LAST_ERROR.with(|error| error.replace(0))
}

/// Record a failure for [`elide_transport_last_error`], as the raw OS errno when there is one.
pub(super) fn set_last_error(error: &io::Error) {
  let code = crate::driver::raw_os_error(error).map_or_else(
    || error_code(io::Error::from(error.kind())) as i32,
    |errno| -(1000 + errno),
  );
  LAST_ERROR.with(|last| last.set(code));
}

struct Accepted {
  socket: Option<Socket>,
  workload: Workload,
  quota: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Drop for Accepted {
  fn drop(&mut self) {
    self.quota.fetch_sub(1, Ordering::Relaxed);
  }
}
static ACCEPTED: LazyLock<Mutex<IntMap<u64, Accepted>>> = LazyLock::new(Mutex::default);

/// Completion layout used by both Java bindings (40 bytes on supported 64-bit platforms).
#[repr(C)]
pub struct NativeEvent {
  /// Operation identity within the driver.
  pub operation: u64,
  /// Originating socket handle.
  pub socket: u64,
  /// Received buffer or transferable accepted socket; zero for other events.
  pub value: u64,
  /// Byte count or zero on success; negative portable error on failure.
  pub result: i64,
  /// 1 connect, 2 accept, 3 receive, 4 send, 5 HTTP request (value = exchange), 6 closed,
  /// 7 HTTP body segment (operation = segment, value = exchange, result = length),
  /// 8 HTTP body end (value = exchange, result = 0 or a negative portable error),
  /// 9 HTTP response part sent (value = exchange, result = bytes or a negative portable error).
  pub kind: u32,
  /// Reserved, always zero.
  pub reserved: u32,
}

/// A receive admission result (32 bytes on supported 64-bit platforms).
#[repr(C)]
#[derive(Default)]
pub struct ReceiveResult {
  /// Nonzero only for a pending receive, completed through ordinary driver polling.
  pub operation: u64,
  /// Newly owned mutable handle for immediate positive bytes; otherwise zero.
  pub buffer: u64,
  /// Initialized storage valid until this handle is frozen, submitted, or released.
  pub address: *mut std::ffi::c_void,
  /// Immediate byte count, EOF zero, or negative portable error; zero while pending.
  pub result: i64,
}

pub(super) fn address(handle: u64) -> Option<SocketAddr> {
  let buffers = lock(registry(handle));
  let Storage::Frozen(buffer) = buffers.get(&handle)? else {
    return None;
  };
  let bytes = buffer.as_ref();
  if bytes.len() != 24 {
    return None;
  }
  let port = u16::from_ne_bytes(bytes[16..18].try_into().ok()?);
  match u16::from_ne_bytes(bytes[18..20].try_into().ok()?) {
    4 => Some(SocketAddr::new(
      Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).into(),
      port,
    )),
    6 => Some(SocketAddr::V6(SocketAddrV6::new(
      Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[..16]).ok()?),
      port,
      0,
      u32::from_ne_bytes(bytes[20..24].try_into().ok()?),
    ))),
    _ => None,
  }
}

fn connect_address(handle: u64) -> Option<socket2::SockAddr> {
  if let Some(ip) = address(handle) {
    return Some(ip.into());
  }
  #[cfg(unix)]
  {
    use std::os::unix::ffi::OsStrExt;
    let buffers = lock(registry(handle));
    let Storage::Frozen(buffer) = buffers.get(&handle)? else {
      return None;
    };
    let bytes = buffer.as_ref();
    if bytes.len() <= 24
      || bytes[..18].iter().any(|byte| *byte != 0)
      || bytes[18..20] != 1u16.to_ne_bytes()
      || bytes[20..24] != [0; 4]
      || bytes[24..].contains(&0)
    {
      return None;
    }
    socket2::SockAddr::unix(std::ffi::OsStr::from_bytes(&bytes[24..])).ok()
  }
  #[cfg(not(unix))]
  None
}

fn encode_address(address: SocketAddr) -> [u8; 24] {
  let mut bytes = [0; 24];
  match address.ip() {
    IpAddr::V4(ip) => {
      bytes[..4].copy_from_slice(&ip.octets());
      bytes[18..20].copy_from_slice(&4u16.to_ne_bytes());
    }
    IpAddr::V6(ip) => {
      bytes[..16].copy_from_slice(&ip.octets());
      bytes[18..20].copy_from_slice(&6u16.to_ne_bytes());
    }
  }
  bytes[16..18].copy_from_slice(&address.port().to_ne_bytes());
  if let SocketAddr::V6(address) = address {
    bytes[20..24].copy_from_slice(&address.scope_id().to_ne_bytes());
  }
  bytes
}

/// Bind a listener using a frozen 24-byte IP endpoint descriptor. Zero indicates failure.
pub fn elide_transport_socket_listen(workload: u64, driver: u64, endpoint: u64, backlog: i32, reuse: u32) -> u64 {
  LAST_ERROR.with(|error| error.set(INVALID));
  if reuse > 1 {
    return 0;
  }
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  let Some(address) = address(endpoint) else {
    return 0;
  };
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    let socket = match state.driver.listen_configured(address, backlog, reuse == 1) {
      Ok(socket) => socket,
      Err(error) => {
        LAST_ERROR.with(|last| last.set(error_code(error) as i32));
        return 0;
      }
    };
    let id = identity();
    if id != 0 {
      state.sockets.insert(id, socket);
      state.workloads.insert(id, workload);
      LAST_ERROR.with(|error| error.set(0));
    }
    id
  })
}

/// Start a connect; its event identifies the newly returned socket. Zero indicates setup failure.
pub fn elide_transport_socket_connect(workload: u64, driver: u64, endpoint: u64) -> u64 {
  LAST_ERROR.with(|error| error.set(INVALID));
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  let Some(address) = connect_address(endpoint) else {
    return 0;
  };
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    let id = identity();
    if id == 0 {
      return 0;
    }
    let (socket, operation) = match state.driver.connect_endpoint(address) {
      Ok(connection) => connection,
      Err(error) => {
        LAST_ERROR.with(|last| last.set(error_code(error) as i32));
        return 0;
      }
    };
    state.sockets.insert(id, socket);
    state.workloads.insert(id, workload);
    state.operations.insert(operation, Operation::raw(id, 0));
    LAST_ERROR.with(|error| error.set(0));
    id
  })
}

/// Request one accept. Zero indicates invalid ownership or admission failure. Accepted sockets
/// belong to the listener's workload.
pub fn elide_transport_socket_accept(workload: u64, driver: u64, listener: u64) -> u64 {
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    if !owned(state, listener, workload) {
      return 0;
    }
    let Some(socket) = state.sockets.get(&listener) else {
      return 0;
    };
    let Ok(operation) = state.driver.accept(socket) else {
      return 0;
    };
    state.operations.insert(operation, Operation::raw(listener, 0));
    operation
  })
}

/// Adopt an accepted socket on its target driver thread under the listener's workload. A different
/// workload leaves the socket with the caller; any other failure consumes and closes it.
pub fn elide_transport_socket_adopt(workload: u64, driver: u64, accepted: u64) -> i32 {
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return INVALID;
    };
    if state.serving.is_some() && state.sockets.contains_key(&accepted) {
      return if owned(state, accepted, workload) { 0 } else { INVALID };
    }
    let mut socket = {
      let mut pending = lock(&ACCEPTED);
      if pending
        .get(&accepted)
        .is_none_or(|socket| socket.workload.id != workload)
      {
        return INVALID;
      }
      pending.remove(&accepted).unwrap()
    };
    if socket.workload.closed() {
      return INVALID;
    }
    let Ok(connection) = state.driver.attach(socket.socket.take().unwrap()) else {
      return INVALID;
    };
    state.sockets.insert(accepted, connection);
    state.workloads.insert(accepted, socket.workload.clone());
    0
  })
}

/// Close an accepted socket before adoption, from any thread.
pub fn elide_transport_socket_discard(accepted: u64) -> i32 {
  if lock(&ACCEPTED).remove(&accepted).is_some() {
    0
  } else {
    INVALID
  }
}

/// Return a local or remote endpoint in a mutable buffer of at least 24 bytes.
pub fn elide_transport_socket_address(driver: u64, socket: u64, peer: u32, output: u64) -> i32 {
  DRIVERS.with(|drivers| {
    let drivers = drivers.borrow();
    let Some(state) = drivers.0.get(&driver) else {
      return INVALID;
    };
    let Some(socket) = state.sockets.get(&socket) else {
      return INVALID;
    };
    let result = match peer {
      0 => socket.local_address(),
      1 => socket.remote_address(),
      _ => return INVALID,
    };
    let Ok(address) = result else { return INVALID };
    let mut buffers = lock(registry(output));
    let Some(Storage::Mutable(buffer)) = buffers.get_mut(&output) else {
      return INVALID;
    };
    if buffer.write(0, &encode_address(address)).is_ok() {
      0
    } else {
      INVALID
    }
  })
}

/// Transfer a mutable buffer to one receive. Its handle is unavailable until the receive event.
pub fn elide_transport_socket_receive(workload: u64, driver: u64, socket: u64, buffer: u64) -> u64 {
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    if !owned(state, socket, workload) {
      return 0;
    }
    let Some(connection) = state.sockets.get(&socket) else {
      return 0;
    };
    let mut buffers = lock(registry(buffer));
    if !matches!(buffers.get(&buffer), Some(Storage::Mutable(_))) {
      return 0;
    }
    let Some(Storage::Mutable(storage)) = buffers.remove(&buffer) else {
      return 0;
    };
    drop(buffers);
    match state.driver.receive(connection, storage) {
      Ok(operation) => {
        state.operations.insert(operation, Operation::raw(socket, buffer));
        operation
      }
      Err((_, storage)) => {
        lock(registry(buffer)).insert(buffer, Storage::Mutable(storage));
        0
      }
    }
  })
}

/// Allocate private receive storage charged to `owner`; successful completion publishes only
/// initialized bytes. EOF and errors publish no buffer. Storage remains charged until completion or
/// driver teardown.
///
/// # Errors
/// Returns zero for invalid handles/capacity, exhausted allocation budgets, or rejected admission.
pub fn elide_transport_socket_receive_new(workload: u64, driver: u64, socket: u64, owner: u64, capacity: u64) -> u64 {
  let Ok(capacity) = usize::try_from(capacity) else {
    return 0;
  };
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    if !owned(state, socket, workload) {
      return 0;
    }
    let Some(connection) = state.sockets.get(&socket) else {
      return 0;
    };
    let Some(budget) = budget(owner) else {
      return 0;
    };
    let Ok(storage) = Buffer::receive(capacity, budget) else {
      return 0;
    };
    match state.driver.receive(connection, storage) {
      Ok(operation) => {
        state.operations.insert(operation, Operation::raw(socket, 0));
        operation
      }
      Err(_) => 0,
    }
  })
}

/// Allocate and submit a receive, returning an immediate result without queueing a completion.
/// Returns zero while pending, one for immediate bytes/EOF/error, or INVALID on admission failure.
/// Pending storage remains charged until native retirement; immediate positive storage is owned
/// by the returned handle and exposes only its initialized byte count.
///
/// # Safety
/// `output` must be aligned and writable. Serialize access to its returned mutable address and
/// abandon it before freezing, submitting, or releasing the returned buffer handle.
pub unsafe fn elide_transport_socket_receive_new_result(
  workload: u64,
  driver: u64,
  socket: u64,
  owner: u64,
  capacity: u64,
  output: *mut ReceiveResult,
) -> i32 {
  if output.is_null() || !output.is_aligned() {
    return INVALID;
  }
  // SAFETY: The caller supplies aligned writable descriptor storage.
  unsafe { output.write(ReceiveResult::default()) };
  let Ok(capacity) = usize::try_from(capacity) else {
    return INVALID;
  };
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return INVALID;
    };
    if !owned(state, socket, workload) {
      return INVALID;
    }
    let Some(connection) = state.sockets.get(&socket) else {
      return INVALID;
    };
    let Some(budget) = budget(owner) else {
      return INVALID;
    };
    let Ok(storage) = Buffer::receive(capacity, budget) else {
      return INVALID;
    };
    match state.driver.receive_with_result(connection, storage) {
      Ok(crate::driver::ReceiveSubmission::Pending(operation)) => {
        state.operations.insert(operation, Operation::raw(socket, 0));
        // SAFETY: Validated output, and no handle or foreign view is published while pending.
        unsafe { (*output).operation = operation };
        0
      }
      Ok(crate::driver::ReceiveSubmission::Complete { result, buffer, .. }) => {
        let mut descriptor = ReceiveResult {
          result: result.map_or_else(error_code, |size| size as i64),
          ..ReceiveResult::default()
        };
        if descriptor.result > 0 {
          let handle = identity();
          if handle == 0 {
            return INVALID;
          }
          let mut buffers = lock(registry(handle));
          buffers.insert(handle, Storage::Mutable(buffer.into_initialized()));
          let Some(Storage::Mutable(storage)) = buffers.get_mut(&handle) else {
            unreachable!()
          };
          descriptor.buffer = handle;
          // Derive the address last, after initialization and registry insertion. No later
          // mutable reborrow of this storage may invalidate the foreign view before return.
          descriptor.address = storage.buf_mut_ptr().cast();
        }
        // SAFETY: Validated output; positive storage now belongs to its returned handle.
        unsafe { output.write(descriptor) };
        1
      }
      Err(_) => INVALID,
    }
  })
}

/// Lease a frozen buffer region for a send. The original handle remains independently owned.
pub fn elide_transport_socket_send(
  workload: u64,
  driver: u64,
  socket: u64,
  buffer: u64,
  offset: u64,
  length: u64,
) -> u64 {
  let (Ok(offset), Ok(length)) = (usize::try_from(offset), usize::try_from(length)) else {
    return 0;
  };
  let Some(end) = offset.checked_add(length) else {
    return 0;
  };
  let storage = {
    let buffers = lock(registry(buffer));
    let Some(Storage::Frozen(storage)) = buffers.get(&buffer) else {
      return 0;
    };
    let Ok(storage) = storage.slice(offset..end) else {
      return 0;
    };
    storage
  };
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    if !owned(state, socket, workload) {
      return 0;
    }
    let Some(connection) = state.sockets.get(&socket) else {
      return 0;
    };
    match state.driver.send(connection, storage) {
      Ok(operation) => {
        state.operations.insert(operation, Operation::raw(socket, 0));
        operation
      }
      Err(_) => 0,
    }
  })
}

/// Try one nonblocking polling-backend send. Returns bytes sent, zero to use asynchronous
/// submission (unsupported backend or backpressure), or a negative transport error.
/// No operation/completion is created, and no source address is retained after return.
///
/// # Safety
/// `source` must point to `length` initialized bytes without concurrent mutation until return.
pub unsafe fn elide_transport_socket_send_inline(
  workload: u64,
  driver: u64,
  socket: u64,
  source: *const u8,
  length: u64,
) -> i64 {
  let Ok(length) = usize::try_from(length) else {
    return -1;
  };
  if source.is_null() || length == 0 || length > 128 * 1024 {
    return -1;
  }
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return -1;
    };
    if !owned(state, socket, workload) {
      return -1;
    }
    let Some(connection) = state.sockets.get(&socket) else {
      return -1;
    };
    // SAFETY: The caller supplies initialized bytes, borrowed only during the nonblocking call.
    let bytes = unsafe { std::slice::from_raw_parts(source, length) };
    state
      .driver
      .try_send(connection, bytes)
      .map_or_else(error_code, |sent| sent.unwrap_or(0) as i64)
  })
}

/// Lease up to 64 frozen regions for one vectored send. Handles remain independently owned.
/// Each descriptor is a native-endian triple of buffer handle, offset, and length.
/// Descriptors are copied before this call returns; buffer storage stays leased until completion.
///
/// # Safety
/// `regions` must point to `count * 3` initialized, aligned u64 values for the duration of this call.
/// No descriptor or buffer may be mutated concurrently with this call.
pub unsafe fn elide_transport_socket_send_gathered(
  workload: u64,
  driver: u64,
  socket: u64,
  regions: *const u64,
  count: u32,
) -> u64 {
  if count == 0 || count > 64 || regions.is_null() || !regions.is_aligned() {
    return 0;
  }
  // SAFETY: The caller supplies the descriptor range; count is bounded before reading it.
  let regions = unsafe { std::slice::from_raw_parts(regions, count as usize * 3) };
  let mut storage = Vec::with_capacity(count as usize);
  for region in regions.as_chunks::<3>().0 {
    let (handle, offset, length) = (region[0], region[1], region[2]);
    let (Ok(offset), Ok(length)) = (usize::try_from(offset), usize::try_from(length)) else {
      return 0;
    };
    let Some(end) = offset.checked_add(length) else {
      return 0;
    };
    if length == 0 {
      return 0;
    }
    let buffers = lock(registry(handle));
    let Some(Storage::Frozen(buffer)) = buffers.get(&handle) else {
      return 0;
    };
    let Ok(view) = buffer.slice(offset..end) else {
      return 0;
    };
    storage.push(view);
  }
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return 0;
    };
    if !owned(state, socket, workload) {
      return 0;
    }
    let Some(connection) = state.sockets.get(&socket) else {
      return 0;
    };
    match state.driver.send_vectored(connection, storage) {
      Ok(operation) => {
        state.operations.insert(operation, Operation::raw(socket, 0));
        operation
      }
      Err(_) => 0,
    }
  })
}

/// Cancel pending work and close an attached socket; late events retain their original identity.
pub fn elide_transport_socket_close(driver: u64, socket: u64) -> i32 {
  DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return INVALID;
    };
    let Some(connection) = state.sockets.remove(&socket) else {
      return INVALID;
    };
    state.workloads.remove(&socket);
    let _ = connection.shutdown(Shutdown::Both);
    for (operation, pending) in &state.operations {
      if pending.socket == socket {
        state.driver.cancel(*operation);
      }
    }
    0
  })
}

pub(super) fn error_code(error: io::Error) -> i64 {
  match error.kind() {
    io::ErrorKind::ConnectionRefused => -4,
    io::ErrorKind::ConnectionReset => -5,
    io::ErrorKind::TimedOut => -6,
    io::ErrorKind::BrokenPipe | io::ErrorKind::NotConnected => -7,
    io::ErrorKind::PermissionDenied => -8,
    io::ErrorKind::AddrInUse => -9,
    io::ErrorKind::AddrNotAvailable => -10,
    io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => -11,
    io::ErrorKind::ConnectionAborted => -12,
    _ => error
      .raw_os_error()
      .map_or(-3, |errno| -(super::OS_ERROR_BASE + i64::from(errno))),
  }
}

/// Set a supported socket option, rejecting unknown options and invalid values.
pub fn elide_transport_socket_option(driver: u64, socket: u64, option: u32, value: i32) -> i32 {
  DRIVERS.with(|drivers| {
    let drivers = drivers.borrow();
    let Some(state) = drivers.0.get(&driver) else {
      return INVALID;
    };
    let Some(socket) = state.sockets.get(&socket) else {
      return INVALID;
    };
    socket
      .set_option(option, value)
      .map_or_else(|error| error_code(error) as i32, |_| 0)
  })
}

/// Shut down read (0), write (1), or both (2) without releasing operation storage.
pub fn elide_transport_socket_shutdown(driver: u64, socket: u64, direction: u32) -> i32 {
  let direction = match direction {
    0 => Shutdown::Read,
    1 => Shutdown::Write,
    2 => Shutdown::Both,
    _ => return INVALID,
  };
  DRIVERS.with(|drivers| {
    let drivers = drivers.borrow();
    let Some(state) = drivers.0.get(&driver) else {
      return INVALID;
    };
    let Some(socket) = state.sockets.get(&socket) else {
      return INVALID;
    };
    socket
      .shutdown(direction)
      .map_or_else(|error| error_code(error) as i32, |_| 0)
  })
}

/// Poll into a caller-owned mutable native buffer, returning a bounded completion count.
/// `timeout_ns = u64::MAX` waits indefinitely, until I/O or an explicit wake.
///
/// # Safety
/// The batch buffer's foreign views must be quiescent during this call. Received buffers likewise
/// cannot retain foreign readers or writers while native receive ownership is active.
pub unsafe fn elide_transport_driver_poll(driver: u64, timeout_ns: u64, batch: u64, maximum: u32) -> i32 {
  if CALLBACK_POLL.get() {
    return INVALID;
  }
  let mut buffer = {
    let mut buffers = lock(registry(batch));
    let Some(Storage::Mutable(buffer)) = buffers.get_mut(&batch) else {
      return INVALID;
    };
    let Some(bytes) = (maximum as usize).checked_mul(size_of::<NativeEvent>()) else {
      return INVALID;
    };
    if maximum == 0 || buffer.buf_capacity() < bytes {
      return INVALID;
    }
    let Some(Storage::Mutable(buffer)) = buffers.remove(&batch) else {
      return INVALID;
    };
    buffer
  };
  let result = DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return INVALID;
    };
    let maximum = maximum as usize;
    sweep(state);
    super::serving::prepare(state);
    // Requests parsed on an earlier poll that did not fit its batch go first.
    let mut scratch = std::mem::take(&mut state.raw_poll);
    let out = &mut scratch.out;
    while out.len() < maximum {
      let Some(event) = state.http.overflow.pop_front() else {
        break;
      };
      out.push(event);
    }
    let timeout = if out.is_empty() && !super::http::needs_poll(state) {
      if timeout_ns == u64::MAX {
        Duration::MAX
      } else {
        Duration::from_nanos(timeout_ns)
      }
    } else {
      Duration::ZERO
    };
    let Ok(events) = state.driver.poll_batch(timeout, maximum) else {
      return INVALID;
    };
    scratch.events.extend(events);
    for event in scratch.events.drain(..) {
      process_event(state, event, out, &mut scratch.http_events);
    }
    // Re-attempt HTTP sends that were deferred by driver op-limit backpressure. poll_batch
    // has drained completions above, so any slot freed by another socket's completion is now
    // available; a still-saturated driver re-defers without sending.
    super::http::retry_deferred(state, out);
    let count = out.len().min(maximum);
    for (index, event) in out.drain(..count).enumerate() {
      // SAFETY: The buffer owns count event-sized slots; write_unaligned does not assume byte-buffer alignment.
      unsafe {
        buffer
          .buf_mut_ptr()
          .add(index * size_of::<NativeEvent>())
          .cast::<NativeEvent>()
          .write_unaligned(event)
      };
    }
    state.http.overflow.extend(out.drain(..));
    state.raw_poll = scratch;
    // An idle poll is the point at which retained receive storage is no longer earning its budget.
    if count == 0 {
      crate::buffer::pool_trim();
    }
    count as i32
  });
  lock(registry(batch)).insert(batch, Storage::Mutable(buffer));
  result
}

fn process_event(
  state: &mut DriverState,
  event: Event,
  out: &mut Vec<NativeEvent>,
  http_events: &mut std::collections::VecDeque<NativeEvent>,
) {
  let id = match &event {
    Event::Connected { id, .. }
    | Event::Accepted { id, .. }
    | Event::Received { id, .. }
    | Event::PersistentReceived { id, .. }
    | Event::PersistentRetired { id }
    | Event::Sent { id, .. }
    | Event::SentVectored { id, .. } => *id,
  };
  let Operation {
    socket,
    buffer: original_buffer,
    http,
  } = if matches!(&event, Event::PersistentReceived { .. }) {
    state.operations.get(&id).copied().unwrap_or_default()
  } else {
    state.operations.remove(&id).unwrap_or_default()
  };
  let mut result = NativeEvent {
    operation: id,
    socket,
    value: 0,
    result: 0,
    kind: 0,
    reserved: 0,
  };
  match event {
    Event::PersistentRetired { .. } => {
      if http {
        super::http::on_persistent_retired(state, socket, http_events);
        out.extend(http_events.drain(..));
      }
      return;
    }
    Event::PersistentReceived {
      result: status,
      buffer: storage,
      ..
    } => {
      // A terminal multishot CQE with data ends the receive operation, not the stream. The
      // driver rearms it; only a zero-byte result reports EOF to HTTP/TLS.
      if http && super::http::is_http(state, socket) {
        super::http::on_persistent_received(state, socket, status, storage, http_events);
        out.extend(http_events.drain(..));
      }
      return;
    }
    Event::Connected { result: status, .. } => {
      result.kind = 1;
      result.result = status.map_or_else(error_code, |_| 0);
    }
    Event::Accepted { result: status, .. } => {
      result.kind = 2;
      if !super::serving::accepted(state, result.socket) {
        // A successful accept may already be queued when its listener is cancelled.
        // Drop its socket here, before publishing any guest-visible connection.
        return;
      }
      // Accepted sockets inherit the listener's workload; a closed one publishes no connection.
      let workload = state
        .workloads
        .get(&result.socket)
        .filter(|workload| !workload.closed())
        .cloned();
      match status {
        Ok(_) if workload.is_none() => result.result = error_code(io::ErrorKind::BrokenPipe.into()),
        Ok(socket) => {
          let workload = workload.unwrap();
          let id = identity();
          if state.serving.as_ref().is_some_and(|shard| !shard.handoff) && id != 0 {
            match state.driver.attach(socket) {
              Ok(connection) => {
                state.sockets.insert(id, connection);
                state.workloads.insert(id, workload);
                super::serving::connected(state, result.socket, id);
                result.value = id;
              }
              Err(error) => result.result = error_code(error),
            }
          } else if id != 0
            && state
              .accepted
              .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < state.limit).then_some(count + 1)
              })
              .is_ok()
          {
            lock(&ACCEPTED).insert(
              id,
              Accepted {
                socket: Some(socket),
                workload,
                quota: state.accepted.clone(),
              },
            );
            result.value = id;
          } else {
            result.result = -3;
          }
        }
        Err(error) => result.result = error_code(error),
      }
    }
    Event::Received {
      result: status,
      buffer: storage,
      ..
    } => {
      // HTTP provenance survives socket closure; late storage must not become a raw handle.
      if http && !super::http::is_http(state, socket) {
        return;
      }
      if http && super::http::is_http(state, socket) {
        super::http::on_received(state, socket, status, storage, http_events);
        out.extend(http_events.drain(..));
        return;
      }
      result.kind = 3;
      result.result = status.map_or_else(error_code, |size| size as i64);
      result.value = original_buffer;
      if original_buffer != 0 {
        lock(registry(original_buffer)).insert(original_buffer, Storage::Mutable(storage));
      } else if socket != 0 && result.result > 0 {
        let handle = identity();
        if handle == 0 {
          result.result = -3;
        } else {
          lock(registry(handle)).insert(handle, Storage::Mutable(storage.into_initialized()));
          result.value = handle;
        }
      }
    }
    Event::Sent { result: status, .. } => {
      result.kind = 4;
      result.result = status.map_or_else(error_code, |size| size as i64);
    }
    Event::SentVectored {
      result: status,
      buffers,
      ..
    } => {
      if http && !super::http::is_http(state, socket) {
        return;
      }
      if http && super::http::is_http(state, socket) {
        super::http::on_sent(state, socket, status, buffers, http_events);
        out.extend(http_events.drain(..));
        return;
      }
      result.kind = 4;
      result.result = status.map_or_else(error_code, |size| size as i64);
    }
  }
  out.push(result);
}

thread_local! {
  static CALLBACK_POLL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct CallbackPoll;

#[derive(Default)]
pub(super) struct PollScratch {
  events: Vec<Event>,
  out: Vec<NativeEvent>,
  http_events: std::collections::VecDeque<NativeEvent>,
  batch: Vec<NativeEvent>,
}

impl Drop for CallbackPoll {
  fn drop(&mut self) {
    CALLBACK_POLL.set(false);
  }
}

/// Poll with owner-thread callbacks, dispatching each completion's parsed events before processing
/// the next completion. No driver borrow or registry lock spans an upcall. A nonzero callback result
/// stops dispatch for this poll; undelivered events remain queued. Recursive polling is rejected, as
/// is a callback registered for a closed workload.
///
/// # Safety
/// The callback and context must remain valid until return. The callback must not unwind and may
/// only borrow the event for its duration; event payloads follow the ordinary poll ownership rules.
pub unsafe fn elide_transport_driver_poll_callback(
  workload: u64,
  driver: u64,
  timeout_ns: u64,
  maximum: u32,
  callback: Option<unsafe extern "C" fn(*mut c_void, *const NativeEvent) -> i32>,
  context: *mut c_void,
) -> i32 {
  let Some(callback) = callback else {
    return INVALID;
  };
  poll_callback(workload, driver, timeout_ns, maximum, 1, |events| {
    // SAFETY: The caller supplies a live callback/context; this event slice stays live until it returns.
    if unsafe { callback(context, events.as_ptr()) } == 0 {
      1
    } else {
      -1
    }
  })
}

/// Poll with batches of ready requests from one socket; other event kinds are offered individually.
/// The callback returns the consumed count, negated to stop even when the whole batch was consumed.
/// Zero or partial consumption also stops. Unconsumed events retain their original order.
///
/// # Safety
/// The callback/context must remain valid until return, and the callback must not unwind. Event
/// storage is borrowed only for the call. Consume at most the offered count; stop immediately after
/// releasing the driver. Payload ownership transfers only for consumed events.
pub unsafe fn elide_transport_driver_poll_batch_callback(
  workload: u64,
  driver: u64,
  timeout_ns: u64,
  maximum: u32,
  callback: Option<unsafe extern "C" fn(*mut c_void, *const NativeEvent, u32) -> i32>,
  context: *mut c_void,
) -> i32 {
  let Some(callback) = callback else {
    return INVALID;
  };
  // SAFETY: The caller supplies a live callback/context; the batch stays live for the synchronous call.
  poll_callback(workload, driver, timeout_ns, maximum, 64, |events| unsafe {
    callback(context, events.as_ptr(), events.len() as u32)
  })
}

fn poll_callback(
  workload: u64,
  driver: u64,
  timeout_ns: u64,
  maximum: u32,
  batch_limit: usize,
  mut callback: impl FnMut(&[NativeEvent]) -> i32,
) -> i32 {
  if maximum == 0 || maximum > i32::MAX as u32 || CALLBACK_POLL.replace(true) {
    return INVALID;
  }
  let _poll = CallbackPoll;
  let scratch = DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let state = drivers.0.get_mut(&driver)?;
    if !state
      .callback_workload
      .as_ref()
      .is_some_and(|owner| owner.admits(workload))
    {
      state.callback_workload = Some(Workload::admit(workload)?);
    }
    sweep(state);
    super::serving::prepare(state);
    let timeout = if !state.http.overflow.is_empty() || super::http::needs_poll(state) {
      Duration::ZERO
    } else if timeout_ns == u64::MAX {
      Duration::MAX
    } else {
      Duration::from_nanos(timeout_ns)
    };
    let events = state.driver.poll_batch(timeout, maximum as usize).ok()?;
    let mut scratch = std::mem::take(&mut state.callback);
    scratch.events.extend(events);
    Some(scratch)
  });
  let Some(mut scratch) = scratch else {
    return INVALID;
  };
  let mut count = 0;
  let mut stopped = false;
  let mut invalid = false;
  let mut dispatch = |outer: Option<&super::http::Cork>| {
    let mut socket = 0;
    let mut cork = None;
    while count < maximum && !stopped {
      let ready = DRIVERS.with(|drivers| {
        let mut drivers = drivers.borrow_mut();
        let state = drivers.0.get_mut(&driver)?;
        let first = state.http.overflow.pop_front()?;
        let socket = first.socket;
        let requests = first.kind == super::http::EVENT_REQUEST;
        scratch.batch.push(first);
        let limit = batch_limit.min((maximum - count) as usize);
        if requests {
          while scratch.batch.len() < limit
            && state
              .http
              .overflow
              .front()
              .is_some_and(|event| event.socket == socket && event.kind == super::http::EVENT_REQUEST)
          {
            scratch.batch.push(state.http.overflow.pop_front().unwrap());
          }
        }
        Some(socket)
      });
      let Some(ready) = ready else {
        break;
      };
      if socket != ready {
        drop(cork.take());
        socket = ready;
        if !outer.is_some_and(|scope| scope.covers(socket)) {
          cork = super::http::Cork::enter(driver, socket);
        }
      }
      let result = callback(&scratch.batch);
      let consumed = result.unsigned_abs() as usize;
      invalid = consumed > scratch.batch.len();
      let consumed = if invalid { 0 } else { consumed };
      count += consumed as u32;
      stopped = invalid || result <= 0 || consumed < scratch.batch.len();
      DRIVERS.with(|drivers| {
        let mut drivers = drivers.borrow_mut();
        if let Some(state) = drivers.0.get_mut(&driver) {
          for event in scratch.batch.drain(consumed..).rev() {
            state.http.overflow.push_front(event);
          }
        } else {
          // Exchange boxes died with the driver. Retirement could not see these temporarily
          // removed events, so release payloads transferred to global registries separately.
          for event in scratch.batch.drain(consumed..) {
            match event.kind {
              2 => {
                elide_transport_socket_discard(event.value);
              }
              3 => {
                elide_transport_buffer_release(event.value);
              }
              super::http::EVENT_BODY => {
                super::http::elide_transport_http_segment_release_retired(event.operation);
              }
              _ => {}
            }
          }
        }
      });
      scratch.batch.clear();
    }
  };
  dispatch(None);
  for event in scratch.events.drain(..) {
    let socket = DRIVERS.with(|drivers| {
      let drivers = drivers.borrow();
      let id = match &event {
        Event::Connected { id, .. }
        | Event::Accepted { id, .. }
        | Event::Received { id, .. }
        | Event::PersistentReceived { id, .. }
        | Event::PersistentRetired { id }
        | Event::Sent { id, .. }
        | Event::SentVectored { id, .. } => *id,
      };
      drivers.0.get(&driver)?.operations.get(&id).map(|op| op.socket)
    });
    let cork = socket.and_then(|socket| super::http::Cork::enter(driver, socket));
    let live = DRIVERS.with(|drivers| {
      let mut drivers = drivers.borrow_mut();
      let Some(state) = drivers.0.get_mut(&driver) else {
        return false;
      };
      process_event(state, event, &mut scratch.out, &mut scratch.http_events);
      state.http.overflow.extend(scratch.out.drain(..));
      true
    });
    if !live {
      return INVALID;
    }
    dispatch(cork.as_ref());
  }
  let live = DRIVERS.with(|drivers| {
    let mut drivers = drivers.borrow_mut();
    let Some(state) = drivers.0.get_mut(&driver) else {
      return false;
    };
    super::http::retry_deferred(state, &mut scratch.out);
    state.http.overflow.extend(scratch.out.drain(..));
    true
  });
  if !live {
    return INVALID;
  }
  dispatch(None);
  DRIVERS.with(|drivers| {
    if let Some(state) = drivers.borrow_mut().0.get_mut(&driver) {
      state.callback = scratch;
    }
  });
  if count == 0 {
    crate::buffer::pool_trim();
  }
  if invalid { INVALID } else { count as i32 }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn unmapped_os_errors_keep_their_errno() {
    let error = io::Error::from_raw_os_error(libc::EAGAIN);
    assert_eq!(
      error_code(error),
      -(super::super::OS_ERROR_BASE + i64::from(libc::EAGAIN))
    );
    assert_eq!(error_code(io::Error::other("no errno")), -3);
    assert_eq!(error_code(io::ErrorKind::ConnectionRefused.into()), -4);
  }
}
