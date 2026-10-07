mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use bemo::abi::*;
use bemo::http::BODY_WINDOW_BYTES;

/// Receive capacity every harness socket is armed with.
const CAPACITY: usize = 16 * 1024;

#[derive(Clone, Copy, Debug)]
struct Ev {
  operation: u64,
  socket: u64,
  value: u64,
  result: i64,
  kind: u32,
}

fn endpoint(owner: u64, address: std::net::SocketAddr) -> u64 {
  let std::net::SocketAddr::V4(v4) = address else {
    panic!("ipv4 endpoint expected")
  };
  let handle = elide_transport_buffer_new(owner, 24);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  let mut bytes = [0u8; 24];
  bytes[..4].copy_from_slice(&v4.ip().octets());
  bytes[16..18].copy_from_slice(&address.port().to_ne_bytes());
  bytes[18..20].copy_from_slice(&4u16.to_ne_bytes());
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.address.cast::<u8>(), 24) };
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(handle, 24) }, 0);
  handle
}

/// Poll until `want` events of `kind` were seen (or other kinds accumulate); returns them.
fn events(driver: u64, batch: u64, kind: u32, want: usize) -> Vec<(u64, u64, i64)> {
  let mut found = Vec::new();
  for _ in 0..64 {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, 200_000_000, batch, 8) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    for index in 0..count as usize {
      // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
      let event = unsafe { &*view.address.cast::<NativeEvent>().add(index) };
      if event.kind == kind {
        found.push((event.socket, event.value, event.result));
      }
    }
    if found.len() >= want {
      return found;
    }
  }
  panic!("wanted {want} events of kind {kind}, saw {}", found.len());
}

fn view(exchange: u64, kind: u32, index: u32) -> Vec<u8> {
  let mut out = [0u64; 2];
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); output is writable or null for validation.
    unsafe { elide_transport_http_view(exchange, kind, index, out.as_mut_ptr()) },
    0
  );
  if out[0] == 0 {
    return Vec::new();
  }
  // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
  unsafe { std::slice::from_raw_parts(out[0] as *const u8, out[1] as usize) }.to_vec()
}

fn respond(driver: u64, exchange: u64, status: u32, headers: &[(&[u8], &[u8])], body: &[u8]) {
  let records: Vec<u64> = headers
    .iter()
    .flat_map(|(n, v)| [n.as_ptr() as u64, n.len() as u64, v.as_ptr() as u64, v.len() as u64])
    .collect();
  assert_eq!(
    // SAFETY: The exchange lease and any header/body storage remain live; null invalid ranges are rejected.
    unsafe {
      elide_transport_http_respond(
        driver,
        exchange,
        status,
        records.as_ptr(),
        headers.len() as u32,
        body.as_ptr(),
        body.len() as u64,
        0,
      )
    },
    0
  );
}

/// Send only the head; the body follows as parts. `body_length` is the declared length or
/// `u64::MAX` for chunked.
fn respond_stream(driver: u64, exchange: u64, status: u32, headers: &[(&[u8], &[u8])], body_length: u64) {
  let records: Vec<u64> = headers
    .iter()
    .flat_map(|(n, v)| [n.as_ptr() as u64, n.len() as u64, v.as_ptr() as u64, v.len() as u64])
    .collect();
  assert_eq!(
    // SAFETY: The exchange lease and any header/body storage remain live; null invalid ranges are rejected.
    unsafe {
      elide_transport_http_respond(
        driver,
        exchange,
        status,
        records.as_ptr(),
        headers.len() as u32,
        std::ptr::null(),
        body_length,
        RESPOND_STREAM,
      )
    },
    0
  );
}

/// Prepare a part, fill it with `payload`, and queue it; returns the send status and the handle.
fn chunk(driver: u64, exchange: u64, payload: &[u8], final_part: bool) -> (i32, u64) {
  let mut address = 0u64;
  // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
  let handle = unsafe { elide_transport_http_chunk_prepare(exchange, payload.len() as u64, &mut address) };
  assert_ne!(handle, 0, "chunk_prepare failed");
  assert_ne!(address, 0);
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), address as *mut u8, payload.len()) };
  let flags = if final_part { CHUNK_FINAL } else { 0 };
  let status = elide_transport_http_chunk_send(driver, exchange, handle, payload.len() as u64, flags);
  (status, handle)
}

fn send_chunk(driver: u64, exchange: u64, payload: &[u8], final_part: bool) {
  assert_eq!(chunk(driver, exchange, payload, final_part).0, 0);
}

/// Split a byte stream at every response head; returns (head, bytes up to the next head).
fn responses(wire: &[u8]) -> Vec<(String, Vec<u8>)> {
  let mut starts: Vec<usize> = wire
    .windows(9)
    .enumerate()
    .filter(|(_, w)| *w == b"HTTP/1.1 " || *w == b"HTTP/1.0 ")
    .map(|(i, _)| i)
    .collect();
  starts.push(wire.len());
  starts
    .windows(2)
    .map(|pair| {
      let message = &wire[pair[0]..pair[1]];
      let split = message.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
      (
        String::from_utf8(message[..split].to_vec()).unwrap(),
        message[split..].to_vec(),
      )
    })
    .collect()
}

struct Harness {
  owner: u64,
  driver: u64,
  socket: u64,
  batch: u64,
  peer: TcpStream,
  /// Events polled but not yet consumed by a `take`.
  pending: Vec<Ev>,
}

impl Harness {
  /// One poll; appends every event to `pending` and returns how many arrived.
  fn poll(&mut self, timeout_ns: u64) -> usize {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(self.driver, timeout_ns, self.batch, 8) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(self.batch, &mut view) }, 0);
    for index in 0..count as usize {
      // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
      let e = unsafe { &*view.address.cast::<NativeEvent>().add(index) };
      self.pending.push(Ev {
        operation: e.operation,
        socket: e.socket,
        value: e.value,
        result: e.result,
        kind: e.kind,
      });
    }
    count as usize
  }

  /// Poll until `want` events of `kind` are pending, then remove and return them in order.
  fn take(&mut self, kind: u32, want: usize) -> Vec<Ev> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
      if self.pending.iter().filter(|e| e.kind == kind).count() >= want {
        break;
      }
      self.poll(200_000_000);
    }
    let (found, rest): (Vec<Ev>, Vec<Ev>) = self.pending.drain(..).partition(|e| e.kind == kind);
    self.pending = rest;
    assert!(found.len() >= want, "wanted {want} of kind {kind}, saw {}", found.len());
    found
  }

  /// Poll until a body-end event for `exchange` is pending; returns its segments in order and
  /// the end event.
  fn take_body(&mut self, exchange: u64) -> (Vec<Ev>, Ev) {
    for _ in 0..64 {
      if self
        .pending
        .iter()
        .any(|e| e.kind == EVENT_BODY_END && e.value == exchange)
      {
        break;
      }
      self.poll(200_000_000);
    }
    let segments = self.take(EVENT_BODY, 0);
    assert!(segments.iter().all(|e| e.value == exchange && e.socket == self.socket));
    let end = self.take(EVENT_BODY_END, 1);
    assert_eq!(end.len(), 1, "one body end expected");
    assert_eq!(end[0].value, exchange);
    (segments, end[0])
  }

  /// Poll until two consecutive polls are silent; returns the kind-7 events pending so far.
  fn drain_segments(&mut self) -> Vec<Ev> {
    let mut quiet = 0;
    while quiet < 2 {
      if self.poll(100_000_000) == 0 {
        quiet += 1;
      } else {
        quiet = 0;
      }
    }
    self.take(EVENT_BODY, 0)
  }

  /// Drive sends while reading the peer until `done` accepts what arrived so far.
  fn read_while_polling(&mut self, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let mut got = Vec::new();
    self.peer.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    for _ in 0..64 {
      self.poll(50_000_000);
      let mut buf = [0u8; 4096];
      if let Ok(n) = self.peer.read(&mut buf) {
        got.extend_from_slice(&buf[..n]);
      }
      if done(&got) {
        break;
      }
    }
    self.peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    got
  }

  fn release_segments(&self, segments: &[Ev]) {
    for s in segments {
      assert_eq!(elide_transport_http_segment_release(self.driver, s.operation), 0);
    }
  }

  fn ack_segments(&self, segments: &[Ev]) {
    for s in segments {
      assert_eq!(elide_transport_http_segment_ack(self.driver, s.operation), 0);
    }
  }

  /// Poll to the body end of `exchange`, acking every segment on arrival but releasing none;
  /// returns them in delivery order. Panics if the body never ends, which is the deadlock a
  /// consumer that only ever releases at collection used to hit.
  fn take_body_acking(&mut self, exchange: u64) -> Vec<Ev> {
    let mut segments = Vec::new();
    for _ in 0..512 {
      let ended = self
        .pending
        .iter()
        .any(|e| e.kind == EVENT_BODY_END && e.value == exchange);
      let arrived = self.take(EVENT_BODY, 0);
      self.ack_segments(&arrived);
      segments.extend(arrived);
      if ended {
        return segments;
      }
      self.poll(100_000_000);
    }
    panic!("body never ended; {} segments acked", segments.len());
  }

  fn finish(mut self) {
    // Let cancelled receives complete so their storage is returned before the driver goes.
    for _ in 0..4 {
      self.poll(20_000_000);
    }
    assert_eq!(elide_transport_buffer_release(self.batch), 0);
    assert_eq!(elide_transport_driver_release(self.driver), 0);
    assert_eq!(elide_transport_owner_used(self.owner), 0);
    assert_eq!(elide_transport_owner_release(self.owner), 0);
  }
}

/// Read a segment's bytes in place through its 16-byte layout, with its data address.
fn segment(handle: u64) -> (u64, Vec<u8>) {
  // SAFETY: The retained body handle begins with two aligned, initialized u64 descriptor words.
  let layout = unsafe { &*(handle as *const [u64; 2]) };
  // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
  let bytes = unsafe { std::slice::from_raw_parts(layout[0] as *const u8, layout[1] as usize) };
  (layout[0], bytes.to_vec())
}

/// Bytes of every segment, concatenated in delivery order.
fn body_of(segments: &[Ev]) -> Vec<u8> {
  segments
    .iter()
    .flat_map(|s| {
      let (_, bytes) = segment(s.operation);
      assert_eq!(bytes.len() as i64, s.result);
      bytes
    })
    .collect()
}

/// The exchange layout's flags byte (offset 31 of 32).
fn flags(exchange: u64) -> u8 {
  // SAFETY: The caller holds the exchange lease; offset 31 lies in its initialized ABI header.
  unsafe { *(exchange as *const u8).add(31) }
}

/// Head address of an exchange; the head is a slice of the receive it arrived in.
fn head_ptr(exchange: u64) -> u64 {
  // SAFETY: The caller holds the exchange lease; the ABI header begins with an aligned u64.
  unsafe { *(exchange as *const u64) }
}

/// Connects a driver socket to a std listener; the std side plays the HTTP client.
fn harness() -> Harness {
  harness_with_context(0)
}

fn harness_with_context(context: u64) -> Harness {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let owner = elide_transport_owner_new(4 * 1024 * 1024);
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 16);
  let endpoint = endpoint(owner, listener.local_addr().unwrap());
  let socket = elide_transport_socket_connect(common::workload(), driver, endpoint);
  assert_ne!(socket, 0);
  let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
  let accepted = std::thread::spawn(move || listener.accept().unwrap().0);
  assert_eq!(events(driver, batch, 1, 1)[0].2, 0);
  let peer = accepted.join().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  assert_eq!(
    if context == 0 {
      elide_transport_socket_http(common::workload(), driver, socket, owner, CAPACITY as u64)
    } else {
      elide_transport_socket_http_tls(common::workload(), driver, socket, owner, CAPACITY as u64, context)
    },
    0
  );
  assert_eq!(elide_transport_buffer_release(endpoint), 0);
  Harness {
    owner,
    driver,
    socket,
    batch,
    peer,
    pending: Vec::new(),
  }
}

struct CallbackState {
  driver: u64,
  batch: u64,
  thread: std::thread::ThreadId,
  requests: usize,
  same_thread: bool,
  statuses: Vec<i32>,
  close: bool,
  stop: bool,
}

unsafe extern "C" fn on_request(context: *mut std::ffi::c_void, event: *const NativeEvent) -> i32 {
  // SAFETY: The polling caller supplied this live, exclusively accessed stack context for the callback.
  let state = unsafe { &mut *(context as *mut CallbackState) };
  // SAFETY: The driver supplies a live initialized event for this synchronous callback.
  let event = unsafe { &*event };
  if event.kind != EVENT_REQUEST {
    return 0;
  }
  state.requests += 1;
  state.same_thread &= std::thread::current().id() == state.thread;
  // Polling recursively would reorder completions; ordinary response/close reentry is legal.
  state
    .statuses
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    .push(unsafe { elide_transport_driver_poll(state.driver, 0, state.batch, 8) });
  if state.close {
    state
      .statuses
      .push(elide_transport_socket_close(state.driver, event.socket));
    state
      .statuses
      .push(elide_transport_http_release(state.driver, event.value));
  } else {
    // SAFETY: The exchange lease and any header/body storage remain live; null invalid ranges are rejected.
    state.statuses.push(unsafe {
      elide_transport_http_respond(
        state.driver,
        event.value,
        200,
        std::ptr::null(),
        0,
        b"callback".as_ptr(),
        8,
        0,
      )
    });
  }
  state
    .statuses
    .push(elide_transport_http_free(state.driver, event.value));
  i32::from(state.stop)
}

fn callback_request(close: bool) {
  let mut h = harness();
  let mut state = CallbackState {
    driver: h.driver,
    batch: h.batch,
    thread: std::thread::current().id(),
    requests: 0,
    same_thread: true,
    statuses: Vec::new(),
    close,
    stop: false,
  };
  h.peer
    .write_all(b"GET /callback HTTP/1.1\r\nHost: test\r\n\r\n")
    .unwrap();
  let deadline = Instant::now() + Duration::from_secs(5);
  while state.requests == 0 && Instant::now() < deadline {
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    let count = unsafe {
      elide_transport_driver_poll_callback(
        common::workload(),
        h.driver,
        50_000_000,
        8,
        Some(on_request),
        &mut state as *mut CallbackState as *mut std::ffi::c_void,
      )
    };
    assert!(count >= 0);
  }
  assert_eq!(state.requests, 1, "request must execute before native poll returns");
  assert!(state.same_thread);
  assert_eq!(state.statuses, if close { vec![-1, 0, 0, 0] } else { vec![-1, 0, 0] });
  if !close {
    let wire = h.read_while_polling(|wire| wire.ends_with(b"callback"));
    assert_eq!(responses(&wire)[0].1, b"callback");
    elide_transport_socket_close(h.driver, h.socket);
  }
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn callback_poll_responds_reentrantly_on_owner_thread() {
  callback_request(false);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn callback_poll_closes_reentrantly_on_owner_thread() {
  callback_request(true);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn callback_poll_retains_pipeline_after_callback_stops() {
  let mut h = harness();
  let mut state = CallbackState {
    driver: h.driver,
    batch: h.batch,
    thread: std::thread::current().id(),
    requests: 0,
    same_thread: true,
    statuses: Vec::new(),
    close: false,
    stop: true,
  };
  h.peer.write_all(b"GET /1 HTTP/1.1\r\nHost: test\r\n\r\nGET /2 HTTP/1.1\r\nHost: test\r\n\r\nGET /3 HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
  let deadline = Instant::now() + Duration::from_secs(5);
  while state.requests < 3 && Instant::now() < deadline {
    let previous = state.requests;
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    let count = unsafe {
      elide_transport_driver_poll_callback(
        common::workload(),
        h.driver,
        50_000_000,
        8,
        Some(on_request),
        &mut state as *mut CallbackState as *mut std::ffi::c_void,
      )
    };
    assert!(count >= 0);
    assert!(state.requests <= previous + 1, "stop must end this callback batch");
  }
  assert_eq!(state.requests, 3);
  assert_eq!(state.statuses, [-1, 0, 0, -1, 0, 0, -1, 0, 0]);
  let wire = h.read_while_polling(|wire| wire.windows(8).filter(|w| *w == b"callback").count() == 3);
  assert_eq!(responses(&wire).len(), 3);
  assert_eq!(elide_transport_socket_close(h.driver, h.socket), 0);
  h.finish();
}

fn callback_batch_request(stop: bool) {
  struct BatchState {
    callback: CallbackState,
    offered: Vec<u32>,
    stop: bool,
  }
  unsafe extern "C" fn consume(context: *mut std::ffi::c_void, events: *const NativeEvent, count: u32) -> i32 {
    // SAFETY: The polling caller supplied this live, exclusively accessed stack context for the callback.
    let state = unsafe { &mut *(context as *mut BatchState) };
    state.offered.push(count);
    let consumed = if state.stop { 1 } else { count };
    for i in 0..consumed {
      // SAFETY: The loop stays within the batch and passes the live nested callback context synchronously.
      unsafe {
        on_request(
          &mut state.callback as *mut CallbackState as *mut std::ffi::c_void,
          events.add(i as usize),
        )
      };
    }
    if state.stop {
      -(consumed as i32)
    } else {
      consumed as i32
    }
  }
  let mut h = harness();
  let mut state = BatchState {
    callback: CallbackState {
      driver: h.driver,
      batch: h.batch,
      thread: std::thread::current().id(),
      requests: 0,
      same_thread: true,
      statuses: Vec::new(),
      close: false,
      stop: false,
    },
    offered: Vec::new(),
    stop,
  };
  h.peer.write_all(b"GET /1 HTTP/1.1\r\nHost: test\r\n\r\nGET /2 HTTP/1.1\r\nHost: test\r\n\r\nGET /3 HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
  let deadline = Instant::now() + Duration::from_secs(5);
  while state.callback.requests < 3 && Instant::now() < deadline {
    let previous = state.callback.requests;
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    let count = unsafe {
      elide_transport_driver_poll_batch_callback(
        common::workload(),
        h.driver,
        50_000_000,
        8,
        Some(consume),
        &mut state as *mut BatchState as *mut std::ffi::c_void,
      )
    };
    assert!(count >= 0);
    if stop {
      assert!(state.callback.requests <= previous + 1);
    }
  }
  assert_eq!(state.callback.requests, 3);
  assert!(
    state.offered.iter().any(|count| *count > 1),
    "pipeline must cross as a batch"
  );
  assert!(state.callback.same_thread);
  assert_eq!(state.callback.statuses, [-1, 0, 0, -1, 0, 0, -1, 0, 0]);
  let wire = h.read_while_polling(|wire| wire.windows(8).filter(|w| *w == b"callback").count() == 3);
  assert_eq!(responses(&wire).len(), 3);
  assert_eq!(elide_transport_socket_close(h.driver, h.socket), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "opens sockets and a driver")]
fn callback_batch_preserves_partial_consumption_and_reentry() {
  callback_batch_request(true);
}

#[test]
#[cfg_attr(miri, ignore = "opens sockets and a driver")]
fn callback_batch_consumes_pipeline_in_one_upcall() {
  callback_batch_request(false);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn callback_poll_revalidates_after_driver_release() {
  unsafe extern "C" fn release(context: *mut std::ffi::c_void, event: *const NativeEvent) -> i32 {
    // SAFETY: The polling caller supplied this live, exclusively accessed stack context for the callback.
    let state = unsafe { &mut *(context as *mut (u64, Vec<i32>)) };
    // SAFETY: The driver supplies a live initialized event for this synchronous callback.
    let event = unsafe { &*event };
    state.1.push(event.result as i32);
    state
      .1
      // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
      .push(unsafe { elide_transport_driver_poll_callback(common::workload(), state.0, 0, 1, Some(release), context) });
    state.1.push(elide_transport_socket_close(state.0, event.socket));
    state.1.push(elide_transport_driver_release(state.0));
    0
  }
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let owner = elide_transport_owner_new(4096);
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 16);
  let address = endpoint(owner, listener.local_addr().unwrap());
  assert_ne!(elide_transport_socket_connect(common::workload(), driver, address), 0);
  elide_transport_buffer_release(address);
  let peer = std::thread::spawn(move || listener.accept().unwrap().0);
  let mut state = (driver, Vec::<i32>::new());
  let deadline = Instant::now() + Duration::from_secs(5);
  while state.1.is_empty() && Instant::now() < deadline {
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    let status = unsafe {
      elide_transport_driver_poll_callback(
        common::workload(),
        driver,
        50_000_000,
        8,
        Some(release),
        &mut state as *mut (u64, Vec<i32>) as *mut std::ffi::c_void,
      )
    };
    assert_eq!(status, if state.1.is_empty() { 0 } else { -1 });
  }
  assert_eq!(state.1, [0, -1, 0, 0]);
  drop(peer.join().unwrap());
  assert_eq!(elide_transport_driver_backend(driver), -1);
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn callback_batch_releases_unconsumed_requests_with_driver() {
  struct ReleaseState {
    driver: u64,
    socket: u64,
    offered: u32,
    released: bool,
  }

  unsafe extern "C" fn release(context: *mut std::ffi::c_void, events: *const NativeEvent, count: u32) -> i32 {
    // SAFETY: The polling caller supplied this live, exclusively accessed stack context for the callback.
    let state = unsafe { &mut *(context as *mut ReleaseState) };
    // SAFETY: The driver supplies a live initialized event for this synchronous callback.
    let first = unsafe { &*events };
    if first.kind != EVENT_REQUEST {
      return count as i32;
    }
    assert!(count > 1, "pipeline must leave an unconsumed request in the batch");
    state.offered = count;
    assert_eq!(elide_transport_http_release(state.driver, first.value), 0);
    assert_eq!(elide_transport_http_free(state.driver, first.value), 0);
    assert_eq!(elide_transport_socket_close(state.driver, state.socket), 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
      match elide_transport_driver_release(state.driver) {
        0 => {
          state.released = true;
          break;
        }
        BUSY => std::thread::yield_now(),
        status => panic!("driver release failed: {status}"),
      }
    }
    assert!(state.released, "driver did not retire during the callback");
    1
  }

  let mut h = harness();
  let mut state = ReleaseState {
    driver: h.driver,
    socket: h.socket,
    offered: 0,
    released: false,
  };
  h.peer.write_all(b"GET /1 HTTP/1.1\r\nHost: test\r\n\r\nGET /2 HTTP/1.1\r\nHost: test\r\n\r\nGET /3 HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
  let deadline = Instant::now() + Duration::from_secs(5);
  while !state.released && Instant::now() < deadline {
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    let status = unsafe {
      elide_transport_driver_poll_batch_callback(
        common::workload(),
        h.driver,
        50_000_000,
        8,
        Some(release),
        &mut state as *mut ReleaseState as *mut std::ffi::c_void,
      )
    };
    assert_eq!(status, if state.released { -1 } else { 0 });
  }
  assert!(state.released);
  assert!(state.offered > 1);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

/// `harness()` with a constrained owner budget, for tests that need to pinch it.
fn harness_with_budget(owner_budget: u64) -> Harness {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let owner = elide_transport_owner_new(owner_budget);
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 16);
  let endpoint = endpoint(owner, listener.local_addr().unwrap());
  let socket = elide_transport_socket_connect(common::workload(), driver, endpoint);
  assert_ne!(socket, 0);
  let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
  let accepted = std::thread::spawn(move || listener.accept().unwrap().0);
  assert_eq!(events(driver, batch, 1, 1)[0].2, 0);
  let peer = accepted.join().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  assert_eq!(
    elide_transport_socket_http(common::workload(), driver, socket, owner, CAPACITY as u64),
    0
  );
  assert_eq!(elide_transport_buffer_release(endpoint), 0);
  Harness {
    owner,
    driver,
    socket,
    batch,
    peer,
    pending: Vec::new(),
  }
}

fn read_until(peer: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
  let mut got = Vec::new();
  let mut buf = [0u8; 4096];
  while !got.windows(needle.len()).any(|w| w == needle) {
    let n = peer.read(&mut buf).unwrap();
    assert!(n > 0, "peer closed early: {}", String::from_utf8_lossy(&got));
    got.extend_from_slice(&buf[..n]);
  }
  got
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn pipelined_requests_are_exposed_and_answered_in_order() {
  let mut h = harness();
  h.peer
    .write_all(
      b"GET /first?x=1 HTTP/1.1\r\nHost: a\r\nX-Trace: t1\r\n\r\nPOST /second HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello",
    )
    .unwrap();
  let requests = h.take(EVENT_REQUEST, 2);
  assert_eq!(requests.len(), 2);
  assert!(requests.iter().all(|e| e.socket == h.socket));
  let (first, second) = (requests[0].value, requests[1].value);
  assert_eq!(flags(first) & HAS_BODY, 0);
  assert_eq!(flags(second) & HAS_BODY, HAS_BODY);

  assert_eq!(elide_transport_http_method(first), 0);
  assert_eq!(elide_transport_http_version(first), 1);
  assert_eq!(elide_transport_http_keep_alive(first), 1);
  assert_eq!(elide_transport_http_header_count(first), 2);
  assert_eq!(view(first, VIEW_METHOD, 0), b"GET");
  assert_eq!(view(first, VIEW_PATH, 0), b"/first?x=1");
  assert_eq!(view(first, VIEW_HEADER_NAME, 1), b"X-Trace");
  assert_eq!(view(first, VIEW_HEADER_VALUE, 1), b"t1");
  assert!(view(first, VIEW_BODY, 0).is_empty());
  assert_eq!(elide_transport_http_method(second), 2);
  // Bodies are not views: they arrive as segments the consumer releases one by one.
  assert!(view(second, VIEW_BODY, 0).is_empty());
  let (segments, end) = h.take_body(second);
  assert_eq!(end.result, 0);
  assert_eq!(body_of(&segments), b"hello");
  h.release_segments(&segments);
  assert_eq!(
    elide_transport_http_segment_release(h.driver, segments[0].operation),
    INVALID
  );
  // One head copy plus spans lets a consumer decode lazily without native views.
  let head = view(first, VIEW_HEAD, 0);
  assert!(head.starts_with(b"GET /first?x=1 HTTP/1.1\r\n") && head.ends_with(b"\r\n\r\n"));
  let mut spans = [0u32; 32];
  // SAFETY: The exchange is live (or a rejected sentinel); the declared output capacity fits the array.
  let needed = unsafe { elide_transport_http_spans(first, spans.as_mut_ptr(), 32) };
  assert_eq!(needed, 4 + 2 * 4);
  assert_eq!(&head[spans[0] as usize..spans[1] as usize], b"GET");
  assert_eq!(&head[spans[2] as usize..spans[3] as usize], b"/first?x=1");
  assert_eq!(&head[spans[8] as usize..spans[9] as usize], b"X-Trace");
  assert_eq!(&head[spans[10] as usize..spans[11] as usize], b"t1");
  // SAFETY: The exchange is live (or a rejected sentinel); the declared output capacity fits the array.
  assert_eq!(unsafe { elide_transport_http_spans(first, spans.as_mut_ptr(), 4) }, 12);
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); output is writable or null for validation.
    unsafe { elide_transport_http_view(first, VIEW_HEADER_NAME, 9, [0u64; 2].as_mut_ptr()) },
    INVALID
  );

  // Answer out of order; the wire must still carry first then second.
  respond(h.driver, second, 201, &[], b"two");
  respond(
    h.driver,
    first,
    200,
    &[(b"content-type", b"text/plain"), (b"Content-Length", b"999")],
    b"one",
  );
  // Drive sends until the peer has both responses.
  let got = h.read_while_polling(|got| got.ends_with(b"two"));
  let text = String::from_utf8(got).unwrap();
  let first_at = text.find("HTTP/1.1 200 OK\r\n").expect("first response");
  let second_at = text.find("HTTP/1.1 201 Created\r\n").expect("second response");
  assert!(first_at < second_at, "{text}");
  assert!(text.contains("content-type: text/plain\r\n"));
  assert!(text.contains("content-length: 3\r\n"));
  assert!(!text.contains("999"), "caller framing header must be dropped: {text}");
  assert!(text.contains("server: elide\r\n"));
  assert!(text.contains("\r\ndate: "));
  // A responded exchange is released; releasing twice is refused.
  assert_eq!(elide_transport_http_release(h.driver, first), INVALID);

  // A closing request ends the connection after its response.
  h.peer
    .write_all(b"GET /bye HTTP/1.1\r\nConnection: close\r\n\r\n")
    .unwrap();
  let bye = h.take(EVENT_REQUEST, 1)[0].value;
  assert_eq!(elide_transport_http_keep_alive(bye), 0);
  respond(h.driver, bye, 204, &[], b"");
  let closed = h.take(EVENT_CLOSED, 1);
  assert_eq!(closed[0].socket, h.socket);
  let tail = read_until(&mut h.peer, b"connection: close\r\n");
  assert!(String::from_utf8_lossy(&tail).starts_with("HTTP/1.1 204 No Content"));
  let mut rest = Vec::new();
  assert_eq!(h.peer.read_to_end(&mut rest).unwrap_or(0), 0, "peer must see EOF");
  for exchange in [first, second, bye] {
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  }
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn chunked_upload_streams_zero_copy_segments() {
  let mut h = harness();
  let request: &[u8] = b"POST /up HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
    5\r\nhello\r\n1;ext=1\r\n \r\n6\r\nworld!\r\n0\r\nTrailer: t\r\n\r\n";
  h.peer.write_all(request).unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  assert_eq!(flags(exchange) & HAS_BODY, HAS_BODY);
  let (segments, end) = h.take_body(exchange);
  assert_eq!(end.result, 0);
  assert_eq!(end.operation, 0);
  assert_eq!(body_of(&segments), b"hello world!");
  assert_eq!(segments.len(), 3, "one segment per chunk: {segments:?}");
  // Zero copy: each segment is the wire bytes in place, inside the receive the head came in.
  let head = head_ptr(exchange);
  let head_len = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
  assert_eq!(view(exchange, VIEW_HEAD, 0).len(), head_len);
  let mut cursor = head_len;
  for s in &segments {
    let (data, bytes) = segment(s.operation);
    assert!(
      data >= head && data + bytes.len() as u64 <= head + CAPACITY as u64,
      "segment {data:#x} outside receive allocation at {head:#x}"
    );
    let at = request[cursor..]
      .windows(bytes.len())
      .position(|w| w == bytes.as_slice())
      .expect("segment bytes on the wire");
    assert_eq!(data - head, (cursor + at) as u64, "segment must alias its wire offset");
    cursor += at + bytes.len();
  }
  // Segments outlive the socket and the exchange; the response goes out meanwhile.
  respond(h.driver, exchange, 200, &[], b"ok");
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  let mut got = Vec::new();
  h.peer.read_to_end(&mut got).unwrap();
  assert!(String::from_utf8_lossy(&got).starts_with("HTTP/1.1 200 OK\r\n"));
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  assert_eq!(body_of(&segments), b"hello world!");
  assert!(elide_transport_owner_used(h.owner) >= CAPACITY as u64);
  h.release_segments(&segments);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn body_window_stalls_receives_until_segments_are_released() {
  let mut h = harness();
  let body: Vec<u8> = (0..BODY_WINDOW_BYTES + 64 * 1024).map(|i| (i % 251) as u8).collect();
  let head = format!(
    "POST /big HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n",
    body.len()
  );
  let mut peer = h.peer.try_clone().unwrap();
  let payload = body.clone();
  let writer = std::thread::spawn(move || {
    peer.write_all(head.as_bytes()).unwrap();
    peer.write_all(&payload).unwrap();
  });
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  // Every segment pins a whole receive, so delivery stops once the window of receive capacity
  // is outstanding, well short of the body.
  let first = h.drain_segments();
  let delivered: usize = first.iter().map(|s| s.result as usize).sum();
  assert!(!first.is_empty());
  assert!(delivered < body.len(), "window did not stall: {delivered}");
  assert!(
    first.len() * CAPACITY >= BODY_WINDOW_BYTES,
    "stalled early: {}",
    first.len()
  );
  assert!(h.pending.iter().all(|e| e.kind != EVENT_BODY_END));
  assert_eq!(h.poll(100_000_000), 0, "receives must stay parked");
  let mut got = body_of(&first);
  // Releasing re-arms the socket and the rest of the body streams through.
  h.release_segments(&first);
  let (rest, end) = h.take_body(exchange);
  writer.join().unwrap();
  assert_eq!(end.result, 0);
  got.extend(body_of(&rest));
  assert_eq!(got.len(), body.len());
  assert!(got == body, "body bytes differ");
  h.release_segments(&rest);
  respond(h.driver, exchange, 204, &[], b"");
  let got = h.read_while_polling(|got| got.ends_with(b"\r\n\r\n"));
  assert!(String::from_utf8_lossy(&got).starts_with("HTTP/1.1 204 No Content"));
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn acked_segments_free_the_window_without_freeing_their_bytes() {
  let mut h = harness();
  // Several times the window: unreachable unless every ack re-arms the receive.
  let body: Vec<u8> = (0..BODY_WINDOW_BYTES * 4).map(|i| (i % 251) as u8).collect();
  let head = format!(
    "POST /big HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n",
    body.len()
  );
  let mut peer = h.peer.try_clone().unwrap();
  let payload = body.clone();
  let writer = std::thread::spawn(move || {
    peer.write_all(head.as_bytes()).unwrap();
    peer.write_all(&payload).unwrap();
  });
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  let segments = h.take_body_acking(exchange);
  writer.join().unwrap();
  let end = h.take(EVENT_BODY_END, 1);
  assert_eq!(end[0].result, 0);
  // An ack is not a release: every segment's bytes are still readable, still in place, and still
  // charged to the owner, which is what makes the hand-off zero-copy.
  assert!(
    segments.len() * CAPACITY > BODY_WINDOW_BYTES,
    "body fit in one window: {}",
    segments.len()
  );
  assert_eq!(body_of(&segments), body);
  assert!(elide_transport_owner_used(h.owner) >= BODY_WINDOW_BYTES as u64);
  // Idempotent, and a release still works on an acked segment.
  h.ack_segments(&segments);
  respond(h.driver, exchange, 204, &[], b"");
  let got = h.read_while_polling(|got| got.ends_with(b"\r\n\r\n"));
  assert!(String::from_utf8_lossy(&got).starts_with("HTTP/1.1 204 No Content"));
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.release_segments(&segments);
  assert_eq!(
    elide_transport_http_segment_ack(h.driver, segments[0].operation),
    INVALID
  );
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn segments_neither_acked_nor_released_still_stall_receives() {
  let mut h = harness();
  let body: Vec<u8> = (0..BODY_WINDOW_BYTES * 4).map(|i| (i % 251) as u8).collect();
  let head = format!(
    "POST /big HTTP/1.1\r\nHost: a\r\nContent-Length: {}\r\n\r\n",
    body.len()
  );
  let mut peer = h.peer.try_clone().unwrap();
  let payload = body.clone();
  let writer = std::thread::spawn(move || {
    peer.write_all(head.as_bytes()).unwrap();
    let _ = peer.write_all(&payload);
  });
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  // A consumer that only holds its segments must still throttle: the window is what stops the
  // driver re-arming, so an ack that is never called must never be implied by delivery alone.
  let held = h.drain_segments();
  let delivered: usize = held.iter().map(|s| s.result as usize).sum();
  assert!(delivered < body.len(), "window did not stall: {delivered}");
  for _ in 0..8 {
    assert_eq!(h.poll(50_000_000), 0, "receives must stay parked");
  }
  assert!(h.pending.iter().all(|e| e.kind != EVENT_BODY_END));
  assert!(h.drain_segments().is_empty(), "delivery resumed unasked");
  // Acking alone is enough to restart it, and the held bytes stay valid across the rest.
  h.ack_segments(&held);
  let rest = h.take_body_acking(exchange);
  writer.join().unwrap();
  assert_eq!(h.take(EVENT_BODY_END, 1)[0].result, 0);
  let mut got = body_of(&held);
  got.extend(body_of(&rest));
  assert_eq!(got, body);
  respond(h.driver, exchange, 204, &[], b"");
  h.read_while_polling(|got| got.ends_with(b"\r\n\r\n"));
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.release_segments(&held);
  h.release_segments(&rest);
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn peer_close_mid_upload_ends_body_with_error_then_closes() {
  let mut h = harness();
  h.peer
    .write_all(b"POST /part HTTP/1.1\r\nHost: a\r\nContent-Length: 100\r\n\r\nten bytes!")
    .unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  let segments = h.take(EVENT_BODY, 1);
  assert_eq!(body_of(&segments), b"ten bytes!");
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  let order: Vec<u32> = h.pending.iter().map(|e| e.kind).collect();
  let end = h.take(EVENT_BODY_END, 1)[0];
  assert_eq!(end.value, exchange);
  assert!(end.result < 0, "truncated body must end with an error: {end:?}");
  assert_eq!(order, vec![EVENT_BODY_END], "body end precedes close: {order:?}");
  // Delivered segments stay readable after the socket is gone.
  assert_eq!(body_of(&segments), b"ten bytes!");
  h.release_segments(&segments);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn responding_before_the_body_ends_discards_it_and_closes() {
  let mut h = harness();
  h.peer
    .write_all(b"POST /early HTTP/1.1\r\nHost: a\r\nContent-Length: 40\r\n\r\nfirst part")
    .unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  let segments = h.take(EVENT_BODY, 1);
  respond(h.driver, exchange, 202, &[], b"");
  let end = h.take(EVENT_BODY_END, 1)[0];
  assert!(end.result < 0 && end.value == exchange, "{end:?}");
  // Read the response before a late upload can race closure and reset the peer's receive queue.
  let got = h.read_while_polling(|got| got.windows(4).any(|w| w == b"\r\n\r\n"));
  assert!(String::from_utf8_lossy(&got).starts_with("HTTP/1.1 202 Accepted"));
  assert!(got.ends_with(b"\r\n\r\n"));
  if let Err(error) = h.peer.write_all(b"the rest of the body is ignored") {
    assert!(
      matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe
      ),
      "late upload failed unexpectedly: {error}"
    );
  }
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  assert!(h.pending.iter().all(|e| e.kind != EVENT_BODY));
  h.release_segments(&segments);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn declared_connection_close_ends_a_keep_alive_exchange() {
  let mut h = harness();
  h.peer.write_all(b"GET /cap HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  respond(h.driver, exchange, 413, &[(b"Connection", b"Close")], b"");
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  let mut got = Vec::new();
  h.peer.read_to_end(&mut got).unwrap();
  let text = String::from_utf8(got).unwrap();
  assert!(text.starts_with("HTTP/1.1 413 Payload Too Large\r\n"), "{text}");
  assert_eq!(text.matches("connection: close\r\n").count(), 1, "{text}");
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn malformed_requests_get_400_and_close() {
  let mut h = harness();
  h.peer.write_all(b"BROKEN\x01 REQUEST\r\n\r\n").unwrap();
  let closed = events(h.driver, h.batch, EVENT_CLOSED, 1);
  assert_eq!(closed[0].0, h.socket);
  let got = read_until(&mut h.peer, b"\r\n\r\n");
  let text = String::from_utf8_lossy(&got);
  assert!(text.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{text}");
  assert!(text.contains("connection: close\r\n"), "{text}");
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn head_requests_carry_length_without_body() {
  let mut h = harness();
  h.peer
    .write_all(b"HEAD /x HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n")
    .unwrap();
  let x = events(h.driver, h.batch, EVENT_REQUEST, 1)[0].1;
  assert_eq!(elide_transport_http_method(x), 1);
  respond(h.driver, x, 200, &[], b"twelve bytes");
  events(h.driver, h.batch, EVENT_CLOSED, 1);
  let mut got = Vec::new();
  h.peer.read_to_end(&mut got).unwrap();
  let text = String::from_utf8(got).unwrap();
  assert!(text.contains("content-length: 12\r\n"), "{text}");
  assert!(text.ends_with("\r\n\r\n"), "no body expected: {text}");
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn peer_reset_notifies_while_exchange_is_pending() {
  let mut h = harness();
  h.peer.write_all(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let exchange = events(h.driver, h.batch, EVENT_REQUEST, 1)[0].1;
  socket2::SockRef::from(&h.peer)
    .set_linger(Some(Duration::ZERO))
    .unwrap();
  drop(h.peer);
  assert_eq!(events(h.driver, h.batch, EVENT_CLOSED, 1)[0].0, h.socket);
  assert_eq!(view(exchange, VIEW_PATH, 0), b"/");
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn encoding_failure_abandons_pending_exchange() {
  let mut h = harness();
  h.peer.write_all(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let exchange = events(h.driver, h.batch, EVENT_REQUEST, 1)[0].1;
  let body = vec![0u8; 4 * 1024 * 1024];
  assert_ne!(
    // SAFETY: The exchange lease and any header/body storage remain live; null invalid ranges are rejected.
    unsafe {
      elide_transport_http_respond(
        h.driver,
        exchange,
        200,
        std::ptr::null(),
        0,
        body.as_ptr(),
        body.len() as u64,
        0,
      )
    },
    0
  );
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  assert_eq!(events(h.driver, h.batch, EVENT_CLOSED, 1)[0].0, h.socket);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

/// A failed receive arm must roll back the `http.sockets` insertion so the
/// caller can close or retry the promotion instead of inheriting a permanent
/// entry no completion (none was submitted) and no `socketClose` can reclaim.
///
/// This drives the shipped trigger: driver admission refusal. With the limit
/// saturated by in-flight connects, the first receive inside `socket_http` is
/// refused at `admit`. The promotion returns `INVALID`; freeing admission and
/// retrying must then succeed, proving the failed promotion left no entry
/// behind (the pre-fix wedge would make the retry return `INVALID`).
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn socket_http_admission_refusal_rolls_back_and_allows_retry() {
  // An address that immediately refuses: bind a listener, then release the port.
  let refused = {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
  };
  let owner = elide_transport_owner_new(4 * 1024 * 1024);
  // limit 8: eight in-flight operations saturate admission (pending + completed
  // >= limit), exactly the shipped DRIVER_CAPACITY condition under load.
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 8);
  let endpoint = endpoint(owner, refused);
  let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
  // Eight connects (one of which we will promote) fill every admission slot.
  let mut sockets: Vec<u64> = Vec::with_capacity(8);
  for _ in 0..8 {
    let socket = elide_transport_socket_connect(common::workload(), driver, endpoint);
    assert_ne!(socket, 0);
    sockets.push(socket);
  }
  let wedge = sockets[0];
  // All eight connect operations are in flight, so the receive arm of the
  // promotion is refused by admission. This is the exact shipped wedge path.
  assert_eq!(
    elide_transport_socket_http(common::workload(), driver, wedge, owner, 16 * 1024),
    INVALID
  );
  // Drain the connects (each completes "refused"); admission is now free.
  let connects = events(driver, batch, 1, 8);
  assert_eq!(connects.len(), 8);
  // Promotion retry must succeed: the entry from the failed attempt was rolled
  // back. Pre-fix, this returns INVALID because the wedged entry guarded by
  // `http.sockets.contains_key` blocks the retry without ever arming.
  assert_eq!(
    elide_transport_socket_http(common::workload(), driver, wedge, owner, 16 * 1024),
    0
  );
  // The armed receive on a refused connection completes with an error and tears
  // the socket down through the internal close_socket path (no exchange made).
  assert_eq!(events(driver, batch, EVENT_CLOSED, 1)[0].0, wedge);
  // Close the remaining refused-connect sockets; the wedged one is already gone.
  for socket in sockets {
    if socket != wedge {
      assert_eq!(elide_transport_socket_close(driver, socket), 0);
    }
  }
  assert_eq!(elide_transport_buffer_release(endpoint), 0);
  assert_eq!(elide_transport_buffer_release(batch), 0);
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_driver_release(driver), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

/// The other `arm_receive` failure path: receive storage cannot be reserved
/// against a too-small owner budget. The promotion must still roll back so the
/// socket can be promoted later against a sufficient budget (covers the part of
/// the fix the admission test cannot reach under the shipped configuration).
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn socket_http_budget_refusal_rolls_back_and_allows_retry() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let owner = elide_transport_owner_new(4 * 1024 * 1024);
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 16);
  let endpoint = endpoint(owner, listener.local_addr().unwrap());
  let socket = elide_transport_socket_connect(common::workload(), driver, endpoint);
  assert_ne!(socket, 0);
  let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
  let accepted = std::thread::spawn(move || listener.accept().unwrap().0);
  assert_eq!(events(driver, batch, 1, 1)[0].2, 0);
  let mut peer = accepted.join().unwrap();
  peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  assert_eq!(elide_transport_buffer_release(endpoint), 0);
  // A 1 KiB owner cannot reserve 16 KiB of receive storage, so the arm fails at
  // `Buffer::receive` before any operation is submitted.
  let tiny = elide_transport_owner_new(1024);
  assert_eq!(
    elide_transport_socket_http(common::workload(), driver, socket, tiny, 16 * 1024),
    INVALID
  );
  assert_eq!(elide_transport_owner_release(tiny), 0);
  // The failed promotion rolled the entry back, so retrying against the real
  // budget succeeds. Pre-fix, the wedged entry makes this return INVALID.
  assert_eq!(
    elide_transport_socket_http(common::workload(), driver, socket, owner, 16 * 1024),
    0
  );
  // Drive the now-valid HTTP socket through one close exchange so it cleans up
  // through close_socket and refunds its receive storage.
  peer.write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").unwrap();
  let exchange = events(driver, batch, EVENT_REQUEST, 1)[0].1;
  respond(driver, exchange, 204, &[], b"");
  assert_eq!(events(driver, batch, EVENT_CLOSED, 1)[0].0, socket);
  assert_eq!(elide_transport_http_free(driver, exchange), 0);
  assert_eq!(elide_transport_buffer_release(batch), 0);
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_driver_release(driver), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

/// Poll until `want` events of `kind` were seen (capped at the driver's `limit = 3` admission
/// bound, which is also the maximum `poll_batch` accepts). Returns them.
fn events3(driver: u64, batch: u64, kind: u32, want: usize) -> Vec<(u64, u64, i64)> {
  let mut found = Vec::new();
  for _ in 0..64 {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, 200_000_000, batch, 3) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    for index in 0..count as usize {
      // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
      let event = unsafe { &*view.address.cast::<NativeEvent>().add(index) };
      if event.kind == kind {
        found.push((event.socket, event.value, event.result));
      }
    }
    if found.len() >= want {
      return found;
    }
  }
  panic!("wanted {want} events of kind {kind}, saw {}", found.len());
}

/// Two keep-alive HTTP sockets on a `limit = 3` driver: two armed receives hold two of three
/// operation slots, leaving one free. Responding to the first socket consumes that slot for its
/// send; responding to the second then hits `Driver::admit`'s op-limit gate (`WouldBlock`).
///
/// Before the fix, `pump` treated that `WouldBlock` as fatal: it set `closing`, cleared the
/// queue (dropping the encoded response) and closed the keep-alive socket, so the second peer
/// saw EOF with zero response bytes and an `EVENT_CLOSED`. After the fix, `pump` defers the
/// send (leaving the response queued) and the per-poll sweep retries it once the first send's
/// completion frees a slot, so both peers receive `200 OK` and no `EVENT_CLOSED` fires.
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn op_limit_during_send_keeps_connection_alive() {
  let owner = elide_transport_owner_new(4 * 1024 * 1024);
  let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 3);
  let batch = elide_transport_buffer_new(owner, 3 * size_of::<NativeEvent>() as u64);
  let listener_a = TcpListener::bind("127.0.0.1:0").unwrap();
  let listener_b = TcpListener::bind("127.0.0.1:0").unwrap();
  let endpoint_a = endpoint(owner, listener_a.local_addr().unwrap());
  let endpoint_b = endpoint(owner, listener_b.local_addr().unwrap());
  let socket_a = elide_transport_socket_connect(common::workload(), driver, endpoint_a);
  let socket_b = elide_transport_socket_connect(common::workload(), driver, endpoint_b);
  assert_ne!(socket_a, 0);
  assert_ne!(socket_b, 0);
  let accepted_a = std::thread::spawn(move || listener_a.accept().unwrap().0);
  let accepted_b = std::thread::spawn(move || listener_b.accept().unwrap().0);
  // Drive both connect completions.
  assert!(events3(driver, batch, 1, 2).iter().all(|event| event.2 == 0));
  let mut peer_a = accepted_a.join().unwrap();
  let mut peer_b = accepted_b.join().unwrap();
  for peer in [&mut peer_a, &mut peer_b] {
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
  }
  assert_eq!(
    elide_transport_socket_http(common::workload(), driver, socket_a, owner, 16 * 1024),
    0
  );
  assert_eq!(
    elide_transport_socket_http(common::workload(), driver, socket_b, owner, 16 * 1024),
    0
  );
  assert_eq!(elide_transport_buffer_release(endpoint_a), 0);
  assert_eq!(elide_transport_buffer_release(endpoint_b), 0);

  // Two armed receives fill two of three slots; each peer sends one keep-alive GET.
  peer_a.write_all(b"GET /a HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  peer_b.write_all(b"GET /b HTTP/1.1\r\nHost: b\r\n\r\n").unwrap();
  let requests = events3(driver, batch, EVENT_REQUEST, 2);
  assert_eq!(requests.len(), 2);
  // Match each request event back to its socket (completion order is not guaranteed).
  let exchange_a = requests
    .iter()
    .find(|event| event.0 == socket_a)
    .expect("request on socket A")
    .1;
  let exchange_b = requests
    .iter()
    .find(|event| event.0 == socket_b)
    .expect("request on socket B")
    .1;

  // Respond to A first: its send takes the one free slot. Then respond to B: the send hits
  // WouldBlock admission. Before the fix this closed B and dropped its response.
  respond(driver, exchange_a, 200, &[], b"a");
  respond(driver, exchange_b, 200, &[], b"b");

  // Drive polls until both peers have their responses; assert no keep-alive socket closed.
  let mut got_a = Vec::new();
  let mut got_b = Vec::new();
  let mut closed = 0;
  for _ in 0..64 {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, 100_000_000, batch, 3) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    for index in 0..count as usize {
      // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
      let event = unsafe { &*view.address.cast::<NativeEvent>().add(index) };
      if event.kind == EVENT_CLOSED {
        closed += 1;
      }
    }
    for (peer, got) in [(&mut peer_a, &mut got_a), (&mut peer_b, &mut got_b)] {
      peer.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
      let mut buf = [0u8; 4096];
      if let Ok(n) = peer.read(&mut buf) {
        got.extend_from_slice(&buf[..n]);
      }
    }
    if got_a.ends_with(b"\r\n\r\na") && got_b.ends_with(b"\r\n\r\nb") {
      break;
    }
  }
  assert!(
    got_a.windows(15).any(|w| w == b"HTTP/1.1 200 OK"),
    "peer A: {:?}",
    String::from_utf8_lossy(&got_a)
  );
  assert!(
    got_b.windows(15).any(|w| w == b"HTTP/1.1 200 OK"),
    "peer B: {:?}",
    String::from_utf8_lossy(&got_b)
  );
  assert_eq!(
    closed, 0,
    "no keep-alive socket should close under op-limit backpressure"
  );

  // Drain: shut down both peers so their receives EOF and the driver can release.
  peer_a.shutdown(std::net::Shutdown::Both).unwrap();
  peer_b.shutdown(std::net::Shutdown::Both).unwrap();
  events3(driver, batch, EVENT_CLOSED, 2);
  assert_eq!(elide_transport_buffer_release(batch), 0);
  assert_eq!(elide_transport_driver_release(driver), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn streaming_response_is_chunked_byte_exact_and_holds_the_lane() {
  let mut h = harness();
  h.peer
    .write_all(b"GET /stream HTTP/1.1\r\nHost: a\r\n\r\nGET /after HTTP/1.1\r\nHost: a\r\n\r\n")
    .unwrap();
  let requests = h.take(EVENT_REQUEST, 2);
  let (stream, after) = (requests[0].value, requests[1].value);
  respond_stream(
    h.driver,
    stream,
    200,
    &[(b"content-type", b"text/plain"), (b"Content-Length", b"7")],
    u64::MAX,
  );
  // The pipelined response is complete first, but waits behind the streaming one.
  respond(h.driver, after, 200, &[], b"after");
  send_chunk(h.driver, stream, b"hello", false);
  send_chunk(h.driver, stream, b" world", false);
  let got = h.read_while_polling(|got| got.ends_with(b" world\r\n"));
  let text = String::from_utf8_lossy(&got);
  assert!(text.contains("transfer-encoding: chunked\r\n"), "{text}");
  assert!(!text.contains("content-length"), "{text}");
  assert!(!text.contains("after"), "complete response overtook the stream: {text}");
  send_chunk(h.driver, stream, b"", true);
  let mut got = got;
  got.extend(h.read_while_polling(|got| got.ends_with(b"after")));
  let parsed = responses(&got);
  assert_eq!(parsed.len(), 2, "{}", String::from_utf8_lossy(&got));
  assert_eq!(parsed[0].1, b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
  assert!(parsed[1].0.contains("content-length: 5\r\n"));
  assert_eq!(parsed[1].1, b"after");
  // One part-sent per queued part of the streaming exchange, head included, in order.
  let sent = h.take(EVENT_PART_SENT, 4);
  assert!(sent.iter().all(|e| e.value == stream && e.socket == h.socket));
  let head_len = parsed[0].0.len() as i64;
  assert_eq!(
    sent.iter().map(|e| e.result).collect::<Vec<_>>(),
    vec![head_len, 10, 11, 5]
  );
  assert!(h.pending.iter().all(|e| e.kind != EVENT_PART_SENT));
  // Further parts after the final one are refused.
  let (status, spare) = chunk(h.driver, stream, b"x", true);
  assert_eq!(status, INVALID);
  assert_eq!(elide_transport_buffer_release(spare), 0);
  assert_eq!(elide_transport_http_free(h.driver, stream), 0);
  assert_eq!(elide_transport_http_free(h.driver, after), 0);
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn callback_cork_coalesces_the_initial_pipeline_send() {
  struct Replies {
    driver: u64,
    exchanges: Vec<u64>,
    statuses: Vec<i32>,
  }
  unsafe extern "C" fn reply(context: *mut std::ffi::c_void, event: *const NativeEvent) -> i32 {
    // SAFETY: The polling caller supplied this live, exclusively accessed stack context for the callback.
    let replies = unsafe { &mut *(context as *mut Replies) };
    // SAFETY: The driver supplies a live initialized event for this synchronous callback.
    let event = unsafe { &*event };
    if event.kind == EVENT_REQUEST {
      replies.exchanges.push(event.value);
      // SAFETY: The exchange lease and any header/body storage remain live; null invalid ranges are rejected.
      replies.statuses.push(unsafe {
        elide_transport_http_respond(
          replies.driver,
          event.value,
          200,
          std::ptr::null(),
          0,
          std::ptr::null(),
          3,
          RESPOND_STREAM,
        )
      });
      replies
        .statuses
        .push(chunk(replies.driver, event.value, b"yes", true).0);
    }
    0
  }
  let mut h = harness();
  h.peer.write_all(b"GET /1 HTTP/1.1\r\nHost: test\r\n\r\nGET /2 HTTP/1.1\r\nHost: test\r\n\r\nGET /3 HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
  let mut replies = Replies {
    driver: h.driver,
    exchanges: Vec::new(),
    statuses: Vec::new(),
  };
  let deadline = Instant::now() + Duration::from_secs(5);
  while replies.exchanges.is_empty() && Instant::now() < deadline {
    assert!(
      // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
      unsafe {
        elide_transport_driver_poll_callback(
          common::workload(),
          h.driver,
          50_000_000,
          8,
          Some(reply),
          &mut replies as *mut Replies as *mut std::ffi::c_void,
        )
      } >= 0
    );
  }
  assert_eq!(replies.exchanges.len(), 3);
  assert_eq!(replies.statuses, [0; 6]);
  let sent = h.take(EVENT_PART_SENT, 1);
  assert_eq!(
    sent.len(),
    6,
    "all three heads and bodies must retire in the first send completion"
  );
  let wire = h.read_while_polling(|wire| wire.windows(3).filter(|w| *w == b"yes").count() == 3);
  assert_eq!(responses(&wire).len(), 3);
  for exchange in replies.exchanges {
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  }
  elide_transport_socket_close(h.driver, h.socket);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn complete_pipelined_responses_coalesce_into_one_send() {
  let mut h = harness();
  h.peer
    .write_all(b"GET /0 HTTP/1.1\r\nHost: a\r\n\r\nGET /1 HTTP/1.1\r\nHost: a\r\n\r\nGET /2 HTTP/1.1\r\nHost: a\r\n\r\nGET /3 HTTP/1.1\r\nHost: a\r\n\r\n")
    .unwrap();
  let requests: Vec<u64> = h.take(EVENT_REQUEST, 4).iter().map(|e| e.value).collect();
  // The head goes out at once and occupies the lane until the next poll; everything queued
  // meanwhile must leave in a single vectored send.
  respond_stream(h.driver, requests[0], 200, &[], u64::MAX);
  send_chunk(h.driver, requests[0], b"aaaa", false);
  send_chunk(h.driver, requests[0], b"cc", true);
  for (i, &exchange) in requests[1..].iter().enumerate() {
    respond_stream(h.driver, exchange, 200, &[], 3);
    send_chunk(h.driver, exchange, format!("r{i}!").as_bytes(), true);
  }
  let head = h.take(EVENT_PART_SENT, 1);
  assert_eq!(head[0].value, requests[0]);
  while !h.pending.iter().any(|e| e.kind == EVENT_PART_SENT) {
    h.poll(200_000_000);
  }
  // The poll that reports the first batched part reports the whole batch: eight parts, which
  // is also the harness batch size.
  let batch = h.take(EVENT_PART_SENT, 0);
  assert_eq!(batch.len(), 8, "{batch:?}");
  let order: Vec<u64> = batch.iter().map(|e| e.value).collect();
  let mut expected = vec![requests[0]; 2];
  for &r in &requests[1..] {
    expected.extend([r, r]);
  }
  assert_eq!(order, expected);
  assert_eq!(batch[0].result, 9); // "4\r\naaaa\r\n"
  assert_eq!(batch[1].result, 12); // "2\r\ncc\r\n0\r\n\r\n"
  let got = h.read_while_polling(|got| got.ends_with(b"r2!"));
  let parsed = responses(&got);
  assert_eq!(parsed.len(), 4);
  assert_eq!(parsed[0].1, b"4\r\naaaa\r\n2\r\ncc\r\n0\r\n\r\n");
  for (i, (head, body)) in parsed[1..].iter().enumerate() {
    assert!(head.contains("content-length: 3\r\n"), "{head}");
    assert_eq!(body, format!("r{i}!").as_bytes());
  }
  for exchange in requests {
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  }
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn short_vectored_sends_resume_until_every_part_is_on_the_wire() {
  const PART: usize = 80 * 1024;
  let mut h = harness();
  // Explicit buffer sizes disable autotuning, so a batch this large cannot fit in one sendmsg.
  assert_eq!(elide_transport_socket_option(h.driver, h.socket, 4, 4096), 0);
  h.peer
    .write_all(b"GET /0 HTTP/1.1\r\nHost: a\r\n\r\nGET /1 HTTP/1.1\r\nHost: a\r\n\r\nGET /2 HTTP/1.1\r\nHost: a\r\n\r\nGET /3 HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n")
    .unwrap();
  let requests: Vec<u64> = h.take(EVENT_REQUEST, 4).iter().map(|e| e.value).collect();
  let parts: Vec<Vec<u8>> = (0..6u8).map(|i| vec![b'a' + i; PART]).collect();
  respond_stream(h.driver, requests[0], 200, &[], u64::MAX);
  send_chunk(h.driver, requests[0], &parts[0], false);
  send_chunk(h.driver, requests[0], &parts[1], false);
  send_chunk(h.driver, requests[0], &parts[2], true);
  for (i, &exchange) in requests[1..].iter().enumerate() {
    respond_stream(h.driver, exchange, 200, &[], PART as u64);
    send_chunk(h.driver, exchange, &parts[3 + i], true);
  }
  assert_eq!(h.take(EVENT_PART_SENT, 1)[0].value, requests[0]);
  // The peer is not reading: the 480 KiB batch stops short and must be resumed.
  while !h.pending.iter().any(|e| e.kind == EVENT_PART_SENT) {
    h.poll(200_000_000);
  }
  let early = h.pending.iter().filter(|e| e.kind == EVENT_PART_SENT).count();
  assert!(early < 9, "the batch was never short: {early}");
  let mut peer = h.peer.try_clone().unwrap();
  let reader = std::thread::spawn(move || {
    let mut got = Vec::new();
    peer.read_to_end(&mut got).unwrap();
    got
  });
  let sent = h.take(EVENT_PART_SENT, 9);
  let mut expected = vec![requests[0]; 3];
  for &r in &requests[1..] {
    expected.extend([r, r]);
  }
  assert_eq!(sent.iter().map(|e| e.value).collect::<Vec<_>>(), expected);
  assert!(sent.iter().all(|e| e.result > 0), "{sent:?}");
  h.take(EVENT_CLOSED, 1);
  let got = reader.join().unwrap();
  let parsed = responses(&got);
  assert_eq!(parsed.len(), 4);
  let mut chunked = Vec::new();
  for part in &parts[..3] {
    chunked.extend_from_slice(format!("{:x}\r\n", PART).as_bytes());
    chunked.extend_from_slice(part);
    chunked.extend_from_slice(b"\r\n");
  }
  chunked.extend_from_slice(b"0\r\n\r\n");
  assert!(parsed[0].1 == chunked, "chunked body differs");
  for (i, (_, body)) in parsed[1..].iter().enumerate() {
    assert!(*body == parts[3 + i], "response {} body differs", i + 1);
  }
  assert!(parsed[3].0.contains("connection: close\r\n"));
  for exchange in requests {
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  }
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn response_window_rejects_parts_until_earlier_ones_are_sent() {
  let mut h = harness();
  h.peer.write_all(b"GET /w HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  respond_stream(h.driver, exchange, 200, &[], u64::MAX);
  let big = vec![b'z'; 200 * 1024];
  send_chunk(h.driver, exchange, &big, false);
  let (status, held) = chunk(h.driver, exchange, &big[..100 * 1024], true);
  assert_eq!(status, BUSY);
  // A refused part stays with the caller.
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(held, &mut view) }, 0);
  let mut got = h.read_while_polling(|got| got.len() > 200 * 1024);
  let sent = h.take(EVENT_PART_SENT, 2);
  assert_eq!(sent[1].result, 200 * 1024 + 9);
  assert_eq!(
    elide_transport_http_chunk_send(h.driver, exchange, held, 100 * 1024, CHUNK_FINAL),
    0
  );
  got.extend(h.read_while_polling(|got| got.ends_with(b"0\r\n\r\n")));
  assert_eq!(h.take(EVENT_PART_SENT, 1)[0].result, 100 * 1024 + 9 + 5);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn response_window_counts_chunk_framing_and_final_terminator() {
  for final_part in [false, true] {
    let mut h = harness();
    h.peer.write_all(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
    let exchange = h.take(EVENT_REQUEST, 1)[0].value;
    respond_stream(h.driver, exchange, 200, &[], u64::MAX);
    h.take(EVENT_PART_SENT, 1);
    // Five hex digits, two CRLFs, and (for a final part) the five-byte terminator.
    let payload = vec![b'z'; RESPONSE_WINDOW_BYTES - 9 - if final_part { 5 } else { 0 }];
    let (status, rejected) = chunk(h.driver, exchange, &vec![b'z'; payload.len() + 1], final_part);
    assert_eq!(status, BUSY);
    assert_eq!(elide_transport_buffer_release(rejected), 0);
    send_chunk(h.driver, exchange, &payload, final_part);
    if !final_part {
      send_chunk(h.driver, exchange, b"", false);
      let (status, held) = chunk(h.driver, exchange, b"", true);
      assert_eq!(status, BUSY);
      h.read_while_polling(|got| got.len() >= RESPONSE_WINDOW_BYTES);
      h.take(EVENT_PART_SENT, 2);
      assert_eq!(
        elide_transport_http_chunk_send(h.driver, exchange, held, 0, CHUNK_FINAL),
        0
      );
    }
    h.read_while_polling(|got| got.ends_with(b"0\r\n\r\n"));
    h.take(EVENT_PART_SENT, 1);
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
    h.peer.shutdown(std::net::Shutdown::Both).unwrap();
    h.take(EVENT_CLOSED, 1);
    h.finish();
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn response_window_does_not_charge_bodiless_payloads() {
  let mut h = harness();
  h.peer.write_all(b"HEAD / HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  respond_stream(h.driver, exchange, 200, &[], u64::MAX);
  h.take(EVENT_PART_SENT, 1);
  send_chunk(h.driver, exchange, &vec![b'z'; RESPONSE_WINDOW_BYTES + 1], true);
  assert_eq!(h.take(EVENT_PART_SENT, 1)[0].result, 0);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.peer.shutdown(std::net::Shutdown::Both).unwrap();
  h.take(EVENT_CLOSED, 1);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn declared_length_divergence_fails_the_connection() {
  let mut h = harness();
  h.peer.write_all(b"GET /len HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  respond_stream(h.driver, exchange, 200, &[], 10);
  send_chunk(h.driver, exchange, b"123456", false);
  h.take(EVENT_PART_SENT, 2);
  let got = h.read_while_polling(|got| got.ends_with(b"123456"));
  assert!(String::from_utf8_lossy(&got).contains("content-length: 10\r\n"));
  let (status, _) = chunk(h.driver, exchange, b"789012", true);
  assert!(status < 0 && status != INVALID && status != BUSY, "{status}");
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  let mut rest = Vec::new();
  assert_eq!(h.peer.read_to_end(&mut rest).unwrap_or(0), 0, "peer must see EOF");
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn unknown_length_on_http_10_ends_with_the_connection() {
  let mut h = harness();
  h.peer.write_all(b"GET /old HTTP/1.0\r\n\r\n").unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  respond_stream(h.driver, exchange, 200, &[], u64::MAX);
  send_chunk(h.driver, exchange, b"raw ", false);
  send_chunk(h.driver, exchange, b"bytes", true);
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  let mut got = Vec::new();
  h.peer.read_to_end(&mut got).unwrap();
  let parsed = responses(&got);
  assert!(parsed[0].0.starts_with("HTTP/1.0 200 OK\r\n"));
  assert!(!parsed[0].0.contains("transfer-encoding"), "{}", parsed[0].0);
  assert_eq!(parsed[0].1, b"raw bytes");
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn every_body_ends_once_unless_the_exchange_is_freed_first() {
  let mut h = harness();
  // Answering before any segment arrived still ends the body, with an error.
  h.peer
    .write_all(b"POST /none HTTP/1.1\r\nHost: a\r\nContent-Length: 40\r\n\r\n")
    .unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  assert_eq!(flags(exchange) & HAS_BODY, HAS_BODY);
  assert_eq!(h.poll(100_000_000), 0);
  respond(h.driver, exchange, 202, &[], b"");
  let end = h.take(EVENT_BODY_END, 1)[0];
  assert!(end.value == exchange && end.result < 0, "{end:?}");
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.take(EVENT_CLOSED, 1);
  h.finish();

  // Freeing an unanswered exchange forfeits its body end: the handle is gone.
  let mut h = harness();
  h.peer
    .write_all(b"POST /drop HTTP/1.1\r\nHost: a\r\nContent-Length: 40\r\n\r\nsome")
    .unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  let segments = h.take(EVENT_BODY, 1);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.take(EVENT_CLOSED, 1);
  h.drain_segments();
  assert!(h.pending.iter().all(|e| e.kind != EVENT_BODY_END), "{:?}", h.pending);
  h.release_segments(&segments);
  h.finish();
}

/// A transient `arm_receive` failure leaves the receive lane empty with no completion pending,
/// so a keep-alive socket goes dormant: later requests never surface as `EVENT_REQUEST`, and no
/// `EVENT_CLOSED` ever fires. The fix re-arms the lane from `pump`, the chokepoint every
/// `respond`/`send`/`free`/`abandon`/`on_sent`/`on_received` path drains through, so the moment
/// the failing arm's budget is refunded — here, by freeing an earlier exchange whose head buffer
/// pinned the owner budget — the lane re-arms and the buried request surfaces.
///
/// 48 KiB fits the initial 16 KiB receive plus exactly one 16 KiB re-arm, but the third arm after
/// that overruns the cap by the ~256 B already charged for the poll batch. Each request lands in
/// its own 16 KiB receive (one head per receive, not the single multi-head receive the existing
/// pipeline test exercises), and each unfree exchange pins that buffer against the owner budget —
/// exactly what happens once a client's requests outrun the server's responses under sustained
/// keep-alive pipelining.
#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn arm_receive_failure_leaves_socket_dormant() {
  let mut h = harness_with_budget(48 * 1024);

  h.peer.write_all(b"GET /a HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let a = events(h.driver, h.batch, EVENT_REQUEST, 1)[0].1;
  assert_eq!(view(a, VIEW_PATH, 0), b"/a");

  h.peer.write_all(b"GET /b HTTP/1.1\r\nHost: b\r\n\r\n").unwrap();
  let b = events(h.driver, h.batch, EVENT_REQUEST, 1)[0].1;
  assert_eq!(view(b, VIEW_PATH, 0), b"/b");

  // Answer /a promptly. The exchange is NOT freed yet, so its 16 KiB receive head keeps
  // charging the budget; later we will free it through the answered path, which is the
  // path the fix protects. Abandoning /a would mark the socket `closing` and skip the
  // re-arm entirely, so this prompt-but-unfree pattern is what the test relies on.
  respond(h.driver, a, 204, &[], b"");

  // /a and /b stay pinned, so the post-/b arm for a third 16 KiB buffer returns false;
  // the socket's receive lane is now empty with no completion pending. The peer sends
  // /c while nothing is reading it.
  h.peer.write_all(b"GET /c HTTP/1.1\r\nHost: c\r\n\r\n").unwrap();

  // Poll long enough that any in-flight receive would have surfaced. The dormant socket
  // never re-arms, so /c must not appear yet — pinning the bug condition we are fixing.
  let mut saw_c = false;
  for _ in 0..40 {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(h.driver, 50_000_000, h.batch, 8) };
    assert!(count >= 0, "poll failed");
    let mut batch_view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(h.batch, &mut batch_view) }, 0);
    for index in 0..count as usize {
      // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
      let event = unsafe { &*batch_view.address.cast::<NativeEvent>().add(index) };
      if event.kind == EVENT_REQUEST && view(event.value, VIEW_PATH, 0) == b"/c" {
        saw_c = true;
      }
    }
  }
  assert!(
    !saw_c,
    "third request surfaced before any free — no dormancy to recover from"
  );

  // Free /a (already answered). With the fix, `http_free` runs `pump`, which finds the
  // receive lane empty, retries the arm, and — now that the budget that blocked it has
  // been refunded by the drop of /a's head buffer — succeeds. The lane submits, /c is
  // read, and the parser surfaces it as an `EVENT_REQUEST`. Before the fix this free
  // did not re-arm, leaving /c buried forever along with no later `EVENT_CLOSED`.
  assert_eq!(elide_transport_http_free(h.driver, a), 0);
  let c = events(h.driver, h.batch, EVENT_REQUEST, 1)[0].1;
  assert_eq!(view(c, VIEW_PATH, 0), b"/c");

  // /b and /c were never answered. Freeing each abandons it (which sets `closing`) and
  // runs `pump`, which closes the socket the moment the order queue drains.
  assert_eq!(elide_transport_http_free(h.driver, b), 0);
  assert_eq!(elide_transport_http_free(h.driver, c), 0);
  assert_eq!(events(h.driver, h.batch, EVENT_CLOSED, 1)[0].0, h.socket);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn expect_continue_unblocks_body_and_preserves_pipeline_order() {
  for framing in ["Content-Length: 4", "Transfer-Encoding: chunked"] {
    let mut h = harness();
    h.peer
      .write_all(
        format!("GET /first HTTP/1.1\r\n\r\nPOST /upload HTTP/1.1\r\n{framing}\r\nExpect: 100-Continue\r\n\r\n")
          .as_bytes(),
      )
      .unwrap();
    let requests = h.take(EVENT_REQUEST, 2);
    let first = requests[0].value;
    let upload = requests[1].value;
    h.peer.set_nonblocking(true).unwrap();
    let mut byte = [0];
    assert_eq!(
      h.peer.read(&mut byte).unwrap_err().kind(),
      std::io::ErrorKind::WouldBlock
    );
    h.peer.set_nonblocking(false).unwrap();
    respond(h.driver, first, 200, &[], b"first");
    let wire = h.read_while_polling(|wire| wire.ends_with(b"HTTP/1.1 100 Continue\r\n\r\n"));
    let replies = responses(&wire);
    assert_eq!(replies.len(), 2, "{wire:?}");
    assert_eq!(replies[0].1, b"first");
    assert_eq!(replies[1].0, "HTTP/1.1 100 Continue\r\n\r\n");
    let body = if framing.starts_with("Content") {
      &b"data"[..]
    } else {
      &b"4\r\ndata\r\n0\r\n\r\n"[..]
    };
    h.peer.write_all(body).unwrap();
    let (segments, end) = h.take_body(upload);
    assert_eq!(end.result, 0);
    assert_eq!(body_of(&segments), b"data");
    h.release_segments(&segments);
    respond(h.driver, upload, 200, &[], b"uploaded");
    let wire = h.read_while_polling(|wire| wire.ends_with(b"uploaded"));
    assert_eq!(responses(&wire).len(), 1);
    assert_eq!(elide_transport_http_free(h.driver, first), 0);
    assert_eq!(elide_transport_http_free(h.driver, upload), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn expect_unsupported_is_rejected_without_dispatch_or_body() {
  for expectation in ["fancy", "100-continue, fancy", "100-continue\r\nExpect: fancy"] {
    let mut h = harness();
    h.peer
      .write_all(format!("POST / HTTP/1.1\r\nContent-Length: 4\r\nExpect: {expectation}\r\n\r\n").as_bytes())
      .unwrap();
    let wire = h.read_while_polling(|wire| wire.ends_with(b"\r\n\r\n"));
    assert!(wire.starts_with(b"HTTP/1.1 417 "), "{wire:?}");
    assert!(!h.pending.iter().any(|event| event.kind == EVENT_REQUEST));
    h.take(EVENT_CLOSED, 1);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn drain_finishes_active_upload_but_rejects_following_heads() {
  let mut h = harness();
  h.peer
    .write_all(b"POST /upload HTTP/1.1\r\nContent-Length: 4\r\n\r\n")
    .unwrap();
  let request = h.take(EVENT_REQUEST, 1)[0].value;
  assert_eq!(elide_transport_http_drain(h.driver), 1);
  h.peer.write_all(b"dataGET /late HTTP/1.1\r\n\r\n").unwrap();
  let (segments, end) = h.take_body(request);
  assert_eq!(end.result, 0);
  assert_eq!(body_of(&segments), b"data");
  h.release_segments(&segments);
  assert!(h.take(EVENT_REQUEST, 0).is_empty());
  respond(h.driver, request, 200, &[], b"finished");
  assert_eq!(elide_transport_http_free(h.driver, request), 0);
  let wire = h.read_while_polling(|wire| wire.ends_with(b"finished"));
  assert_eq!(responses(&wire)[0].1, b"finished");
  h.take(EVENT_CLOSED, 1);
  assert_eq!(elide_transport_http_drain(h.driver), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn drain_waits_for_buffered_write_after_request_is_freed() {
  let mut h = harness();
  h.peer.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
  let request = h.take(EVENT_REQUEST, 1)[0].value;
  let payload = vec![b'x'; 128 * 1024];
  respond(h.driver, request, 200, &[], &payload);
  assert_eq!(elide_transport_http_free(h.driver, request), 0);
  assert_eq!(elide_transport_http_drain(h.driver), 1);
  let wire = h.read_while_polling(|wire| {
    wire
      .windows(4)
      .position(|p| p == b"\r\n\r\n")
      .is_some_and(|head| wire.len() == head + 4 + payload.len())
  });
  let head = wire.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
  assert_eq!(&wire[head..], &payload);
  h.take(EVENT_CLOSED, 1);
  assert_eq!(elide_transport_http_drain(h.driver), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn request_admission_pauses_until_an_exchange_is_freed() {
  let mut h = harness();
  h.peer
    .write_all(&b"GET / HTTP/1.1\r\nHost: a\r\n\r\n".repeat(64))
    .unwrap();
  let requests = h.take(EVENT_REQUEST, 64);
  assert_eq!(requests.len(), 64);
  h.peer.write_all(b"GET /later HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let deadline = Instant::now() + Duration::from_millis(100);
  while Instant::now() < deadline {
    h.poll(10_000_000);
  }
  assert!(!h.pending.iter().any(|event| event.kind == EVENT_REQUEST));
  respond(h.driver, requests[0].value, 200, &[], b"ok");
  assert_eq!(elide_transport_http_free(h.driver, requests[0].value), 0);
  let later = h.take(EVENT_REQUEST, 1);
  assert_eq!(view(later[0].value, VIEW_PATH, 0), b"/later");
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn request_admission_limit_keeps_receiving_the_current_body() {
  let mut h = harness();
  let mut input = b"GET / HTTP/1.1\r\nHost: a\r\n\r\n".repeat(63);
  input.extend_from_slice(b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 1\r\n\r\n");
  h.peer.write_all(&input).unwrap();
  let requests = h.take(EVENT_REQUEST, 64);
  h.peer.write_all(b"x").unwrap();
  let (segments, end) = h.take_body(requests.last().unwrap().value);
  assert_eq!(end.result, 0);
  assert_eq!(body_of(&segments), b"x");
  h.release_segments(&segments);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn request_admission_retains_the_rest_of_a_pipelined_receive() {
  let mut h = harness();
  let mut input = b"GET / HTTP/1.1\r\nHost: a\r\n\r\n".repeat(64);
  input.extend_from_slice(b"POST /later HTTP/1.1\r\nHost: a\r\nContent-Length: 1\r\n\r\nx");
  h.peer.write_all(&input).unwrap();
  let requests = h.take(EVENT_REQUEST, 64);
  assert_eq!(requests.len(), 64);
  for _ in 0..3 {
    h.poll(10_000_000);
  }
  assert!(!h.pending.iter().any(|event| event.kind == EVENT_REQUEST));
  respond(h.driver, requests[0].value, 200, &[], b"ok");
  assert_eq!(elide_transport_http_free(h.driver, requests[0].value), 0);
  let later = h.take(EVENT_REQUEST, 1);
  assert_eq!(view(later[0].value, VIEW_PATH, 0), b"/later");
  let (segments, end) = h.take_body(later[0].value);
  assert_eq!(end.result, 0);
  assert_eq!(body_of(&segments), b"x");
  h.release_segments(&segments);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn admission_pause_preserves_multiple_receive_buffers_in_wire_order() {
  check_admission_pipeline(false);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn half_close_preserves_responses_behind_paused_admission() {
  check_admission_pipeline(true);
}

fn check_admission_pipeline(half_close: bool) {
  let mut h = harness();
  let mut input = Vec::new();
  let padding = "x".repeat(1024);
  for index in 0..96 {
    input.extend_from_slice(format!("GET /{index} HTTP/1.1\r\nHost: a\r\nX-Pad: {padding}\r\n\r\n").as_bytes());
  }
  h.peer.write_all(&input).unwrap();
  if half_close {
    h.peer.shutdown(std::net::Shutdown::Write).unwrap();
  }
  let first = h.take(EVENT_REQUEST, 64);
  for _ in 0..3 {
    h.poll(10_000_000);
  }
  assert!(!h.pending.iter().any(|event| event.kind == EVENT_REQUEST));
  for (index, request) in first.iter().enumerate() {
    assert_eq!(view(request.value, VIEW_PATH, 0), format!("/{index}").as_bytes());
    respond(h.driver, request.value, 204, &[], b"");
    assert_eq!(elide_transport_http_free(h.driver, request.value), 0);
  }
  let rest = h.take(EVENT_REQUEST, 32);
  for (index, request) in rest.iter().enumerate() {
    assert_eq!(view(request.value, VIEW_PATH, 0), format!("/{}", index + 64).as_bytes());
    respond(h.driver, request.value, 204, &[], b"");
    assert_eq!(elide_transport_http_free(h.driver, request.value), 0);
  }
  let wire = if half_close {
    assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
    let mut wire = Vec::new();
    h.peer.read_to_end(&mut wire).unwrap();
    wire
  } else {
    h.read_while_polling(|bytes| bytes.windows(4).filter(|end| *end == b"\r\n\r\n").count() == 96)
  };
  assert_eq!(responses(&wire).len(), 96);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn plaintext_half_close_preserves_a_delayed_response() {
  let mut h = harness();
  h.peer.write_all(b"GET /later HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  h.peer.shutdown(std::net::Shutdown::Write).unwrap();
  let request = h.take(EVENT_REQUEST, 1)[0].value;
  for _ in 0..3 {
    h.poll(10_000_000);
  }
  assert!(!h.pending.iter().any(|event| event.kind == EVENT_CLOSED));
  respond(h.driver, request, 200, &[], b"after EOF");
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  let mut wire = Vec::new();
  h.peer.read_to_end(&mut wire).unwrap();
  assert!(wire.ends_with(b"after EOF"));
  assert_eq!(elide_transport_http_free(h.driver, request), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn eof_during_a_streaming_response_disconnects() {
  let mut h = harness();
  h.peer.write_all(b"GET /pending HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
  let request = h.take(EVENT_REQUEST, 1)[0].value;
  respond_stream(h.driver, request, 200, &[], u64::MAX);
  send_chunk(h.driver, request, b"pending", false);
  h.read_while_polling(|got| got.ends_with(b"pending\r\n"));
  h.peer.shutdown(std::net::Shutdown::Write).unwrap();
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  assert_eq!(elide_transport_http_free(h.driver, request), 0);
}

fn tls_fixture() -> (Harness, rustls::ClientConfig) {
  tls_fixture_with_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
}

fn tls_fixture_with_versions(
  versions: &[&'static rustls::SupportedProtocolVersion],
) -> (Harness, rustls::ClientConfig) {
  tls_fixture_with_protocol(versions, b"http/1.1")
}

fn tls_fixture_with_protocol(
  versions: &[&'static rustls::SupportedProtocolVersion],
  protocol: &[u8],
) -> (Harness, rustls::ClientConfig) {
  use rustls::pki_types::{CertificateDer, pem::PemObject};
  use std::sync::Arc;
  let cert = include_bytes!("fixtures/localhost-cert.pem");
  let key = include_bytes!("fixtures/localhost-key.pem");
  let owner = elide_transport_owner_new(65536);
  let frozen = |data: &[u8]| {
    let handle = elide_transport_buffer_new(owner, data.len() as u64);
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
    // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
    unsafe {
      std::ptr::copy_nonoverlapping(data.as_ptr(), view.address.cast::<u8>(), data.len());
    }
    // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
    assert_eq!(unsafe { elide_transport_buffer_freeze(handle, data.len() as u64) }, 0);
    handle
  };
  let chain = frozen(cert);
  let key = frozen(key);
  let mut protocols = vec![protocol.len() as u8];
  protocols.extend_from_slice(protocol);
  let alpn = frozen(&protocols);
  let context = elide_transport_tls_server(common::workload(), chain, key, alpn);
  assert_ne!(context, 0);
  let h = harness_with_context(context);
  assert_eq!(elide_transport_tls_context_release(context), 0);
  for handle in [chain, key, alpn] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_release(owner), 0);
  let mut roots = rustls::RootCertStore::empty();
  roots.add(CertificateDer::from_pem_slice(cert).unwrap()).unwrap();
  let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
  let mut config = rustls::ClientConfig::builder_with_provider(provider)
    .with_protocol_versions(versions)
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
  config.alpn_protocols = vec![protocol.to_vec()];
  (h, config)
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_http_tls_streams_body_response_and_close_notify() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  for version in [&rustls::version::TLS13, &rustls::version::TLS12] {
    let (mut h, config) = tls_fixture_with_versions(&[version]);
    let peer = h.peer.try_clone().unwrap();
    let client = std::thread::spawn(move || {
      let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
      let mut stream = rustls::StreamOwned::new(client, peer);
      stream
        .write_all(b"POST /tls HTTP/1.1\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata")
        .unwrap();
      stream.flush().unwrap();
      let mut reply = Vec::new();
      stream.read_to_end(&mut reply).unwrap();
      assert_eq!(stream.conn.alpn_protocol(), Some(b"http/1.1".as_slice()));
      reply
    });
    let exchange = h.take(EVENT_REQUEST, 1)[0].value;
    let (segments, end) = h.take_body(exchange);
    assert_eq!(end.result, 0);
    assert_eq!(body_of(&segments), b"data");
    respond(h.driver, exchange, 200, &[], &vec![b'x'; 32768]);
    h.take(EVENT_CLOSED, 1);
    let response = client.join().unwrap();
    assert_eq!(responses(&response)[0].1.len(), 32768);
    assert!(responses(&response)[0].1.iter().all(|b| *b == b'x'));
    assert_eq!(body_of(&segments), b"data");
    h.release_segments(&segments);
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
    assert_eq!(elide_transport_driver_release(h.driver), 0);
    assert_eq!(elide_transport_buffer_release(h.batch), 0);
    assert_eq!(elide_transport_owner_used(h.owner), 0);
    assert_eq!(elide_transport_owner_release(h.owner), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn retained_body_storage_outlives_driver_and_releases_on_another_thread() {
  let mut h = harness();
  h.peer
    .write_all(b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\ndata")
    .unwrap();
  let request = h.take(EVENT_REQUEST, 1)[0].value;
  let (segments, end) = h.take_body(request);
  assert_eq!(end.result, 0);
  h.ack_segments(&segments);
  assert_eq!(elide_transport_http_retire(h.driver), 0);
  let deadline = Instant::now() + Duration::from_secs(5);
  while elide_transport_driver_release(h.driver) == BUSY {
    assert!(Instant::now() < deadline);
    h.poll(10_000_000);
  }
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert!(elide_transport_owner_used(h.owner) > 0);
  std::thread::spawn(move || {
    assert_eq!(body_of(&segments), b"data");
    for segment in segments {
      assert_eq!(elide_transport_http_segment_release_retired(segment.operation), 0);
      assert_eq!(elide_transport_http_segment_release_retired(segment.operation), INVALID);
    }
  })
  .join()
  .unwrap();
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_http_tls_rejects_wrong_name_untrusted_chain_and_h2() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  for failure in ["name", "trust", "alpn"] {
    let (mut h, mut config) = tls_fixture();
    if failure == "alpn" {
      config.alpn_protocols = vec![b"h2".to_vec()];
    } else if failure == "trust" {
      config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    }
    let peer = h.peer.try_clone().unwrap();
    let client = std::thread::spawn(move || {
      let name = if failure == "name" {
        "wrong.example"
      } else {
        "localhost"
      };
      let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from(name).unwrap()).unwrap();
      let mut stream = rustls::StreamOwned::new(client, peer);
      assert!(stream.write_all(b"GET / HTTP/1.1\r\n\r\n").is_err());
      let _ = stream.sock.shutdown(std::net::Shutdown::Both);
    });
    h.take(EVENT_CLOSED, 1);
    client.join().unwrap();
    assert!(!h.pending.iter().any(|event| event.kind == EVENT_REQUEST));
    assert_eq!(elide_transport_driver_release(h.driver), 0);
    assert_eq!(elide_transport_buffer_release(h.batch), 0);
    assert_eq!(elide_transport_owner_used(h.owner), 0);
    assert_eq!(elide_transport_owner_release(h.owner), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_http_tls_truncated_upload_reports_body_failure() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  let (mut h, config) = tls_fixture();
  let peer = h.peer.try_clone().unwrap();
  let client = std::thread::spawn(move || {
    let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
    let mut stream = rustls::StreamOwned::new(client, peer);
    stream
      .write_all(b"POST / HTTP/1.1\r\nContent-Length: 8\r\n\r\ndata")
      .unwrap();
    stream.flush().unwrap();
    stream.sock.shutdown(std::net::Shutdown::Write).unwrap();
  });
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  let (segments, end) = h.take_body(exchange);
  assert!(end.result < 0);
  assert_eq!(body_of(&segments), b"data");
  h.take(EVENT_CLOSED, 1);
  client.join().unwrap();
  h.release_segments(&segments);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_http_tls_streaming_completion_keeps_plaintext_lengths() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  let (mut h, config) = tls_fixture();
  let peer = h.peer.try_clone().unwrap();
  let client = std::thread::spawn(move || {
    let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
    let mut stream = rustls::StreamOwned::new(client, peer);
    stream
      .write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
      .unwrap();
    stream.flush().unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    reply
  });
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  respond_stream(h.driver, exchange, 200, &[], u64::MAX);
  send_chunk(h.driver, exchange, b"hello", true);
  h.take(EVENT_CLOSED, 1);
  let response = client.join().unwrap();
  assert!(response.ends_with(b"5\r\nhello\r\n0\r\n\r\n"));
  let sent = h.take(EVENT_PART_SENT, 2);
  assert_eq!(
    sent.iter().map(|event| event.result as usize).sum::<usize>(),
    response.len()
  );
  assert_eq!(sent[1].result, 15);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_http_tls_drain_closes_idle_handshakes() {
  let (mut h, _) = tls_fixture();
  assert_eq!(elide_transport_http_drain(h.driver), 0);
  h.take(EVENT_CLOSED, 1);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_http_tls_pipeline_resumes_across_admission_and_record_boundaries() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  let (mut h, config) = tls_fixture();
  let peer = h.peer.try_clone().unwrap();
  let client = std::thread::spawn(move || {
    let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
    let mut stream = rustls::StreamOwned::new(client, peer);
    for index in 0..130 {
      let close = if index == 129 { "Connection: close\r\n" } else { "" };
      write!(
        stream,
        "GET /{index} HTTP/1.1\r\nX-Pad: {}\r\n{close}\r\n",
        "x".repeat(256)
      )
      .unwrap();
    }
    stream.flush().unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    reply
  });
  let mut received = 0;
  while received < 130 {
    let requests = h.take(EVENT_REQUEST, 1);
    for request in requests {
      assert_eq!(view(request.value, VIEW_PATH, 0), format!("/{received}").as_bytes());
      respond(h.driver, request.value, 200, &[], b"x");
      assert_eq!(elide_transport_http_free(h.driver, request.value), 0);
      received += 1;
    }
  }
  h.take(EVENT_CLOSED, 1);
  let response = client.join().unwrap();
  assert_eq!(responses(&response).len(), 130);
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_http_tls_large_response_retries_inline_yields_and_backpressure() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  for send_capacity in [4096, 1024 * 1024] {
    let (mut h, config) = tls_fixture();
    assert_eq!(elide_transport_socket_option(h.driver, h.socket, 4, send_capacity), 0);
    assert_eq!(elide_transport_socket_option(h.driver, h.socket, 1, 1), 0);
    let payload: Vec<u8> = (0..2 * 1024 * 1024 + 17)
      .map(|index| ((index * 131 + index / 257) % 251) as u8)
      .collect();
    let peer = h.peer.try_clone().unwrap();
    let (start, wait) = std::sync::mpsc::channel();
    let client = std::thread::spawn(move || {
      let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
      let mut stream = rustls::StreamOwned::new(client, peer);
      stream
        .write_all(b"GET /large HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
      stream.flush().unwrap();
      wait.recv_timeout(Duration::from_secs(5)).unwrap();
      let mut wire = Vec::new();
      stream.read_to_end(&mut wire).unwrap();
      wire
    });
    let exchange = h.take(EVENT_REQUEST, 1)[0].value;
    respond(h.driver, exchange, 200, &[], &payload);
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
    start.send(()).unwrap();
    let expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timeout = expired.clone();
    let driver = h.driver;
    let (finished, waiting) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
      if matches!(
        waiting.recv_timeout(Duration::from_secs(10)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
      ) {
        timeout.store(true, std::sync::atomic::Ordering::Release);
        elide_transport_driver_wake(driver);
      }
    });
    while !h.pending.iter().any(|event| event.kind == EVENT_CLOSED) {
      // No timer or unrelated request may be needed to resume an inline drive yield.
      h.poll(u64::MAX);
      assert!(
        !expired.load(std::sync::atomic::Ordering::Acquire),
        "inline TLS progress stalled"
      );
    }
    drop(finished);
    watchdog.join().unwrap();
    h.take(EVENT_CLOSED, 1);
    let wire = client.join().unwrap();
    let replies = responses(&wire);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].1, payload);
    assert_eq!(elide_transport_driver_release(h.driver), 0);
    assert_eq!(elide_transport_buffer_release(h.batch), 0);
    assert_eq!(elide_transport_owner_used(h.owner), 0);
    assert_eq!(elide_transport_owner_release(h.owner), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn freed_pipeline_handles_cannot_replace_queued_responses() {
  let mut h = harness();
  let mut peer = h.peer.try_clone().unwrap();
  let (start, wait) = std::sync::mpsc::channel();
  let client = std::thread::spawn(move || {
    for index in 0..130 {
      let close = if index == 129 { "Connection: close\r\n" } else { "" };
      write!(peer, "GET /{index} HTTP/1.1\r\n{close}\r\n").unwrap();
    }
    wait.recv().unwrap();
    let mut wire = Vec::new();
    peer.read_to_end(&mut wire).unwrap();
    wire
  });
  let mut received = 0;
  while received < 130 {
    for request in h.take(EVENT_REQUEST, 1) {
      assert_eq!(view(request.value, VIEW_PATH, 0), format!("/{received}").as_bytes());
      let body = if received == 0 {
        vec![b'x'; 1024 * 1024]
      } else {
        vec![b'x']
      };
      respond(h.driver, request.value, 200, &[], &body);
      assert_eq!(elide_transport_http_free(h.driver, request.value), 0);
      received += 1;
      if received == 64 {
        start.send(()).unwrap();
      }
    }
  }
  h.take(EVENT_CLOSED, 1);
  let wire = client.join().unwrap();
  let replies = responses(&wire);
  assert_eq!(replies.len(), 130);
  assert_eq!(replies[0].1.len(), 1024 * 1024);
  assert!(replies[1..].iter().all(|reply| reply.1 == b"x"));
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

struct H2TlsIo<T: Read + Write = TcpStream>(rustls::StreamOwned<rustls::ClientConnection, T>);

impl<T: Read + Write + Unpin> tokio::io::AsyncRead for H2TlsIo<T> {
  fn poll_read(
    mut self: std::pin::Pin<&mut Self>,
    _: &mut std::task::Context<'_>,
    output: &mut tokio::io::ReadBuf<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    match self.0.read(output.initialize_unfilled()) {
      Ok(length) => {
        output.advance(length);
        std::task::Poll::Ready(Ok(()))
      }
      Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::task::Poll::Pending,
      Err(error) => std::task::Poll::Ready(Err(error)),
    }
  }
}
impl<T: Read + Write + Unpin> tokio::io::AsyncWrite for H2TlsIo<T> {
  fn poll_write(
    mut self: std::pin::Pin<&mut Self>,
    _: &mut std::task::Context<'_>,
    bytes: &[u8],
  ) -> std::task::Poll<std::io::Result<usize>> {
    match self.0.write(bytes) {
      Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::task::Poll::Pending,
      result => std::task::Poll::Ready(result),
    }
  }
  fn poll_flush(
    mut self: std::pin::Pin<&mut Self>,
    _: &mut std::task::Context<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    match self.0.flush() {
      Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::task::Poll::Pending,
      result => std::task::Poll::Ready(result),
    }
  }
  fn poll_shutdown(
    mut self: std::pin::Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    self.0.conn.send_close_notify();
    match self.as_mut().poll_flush(cx) {
      std::task::Poll::Ready(Err(error))
        if matches!(
          error.kind(),
          std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
        ) =>
      {
        // H2 can finish on GOAWAY before rustls has read the peer's close_notify.
        // A failed reciprocal write must not prevent authenticating that pending
        // input. Never turn an unverified TCP disconnect into a clean TLS close.
        let stream = &mut self.0;
        loop {
          let state = stream.conn.process_new_packets().map_err(std::io::Error::other)?;
          if state.peer_has_closed() {
            return std::task::Poll::Ready(tls_shutdown_result(Err(error), true));
          }
          if state.plaintext_bytes_to_read() != 0 {
            return std::task::Poll::Ready(Err(error));
          }
          match stream.conn.read_tls(&mut stream.sock) {
            Ok(0) => return std::task::Poll::Ready(Err(error)),
            Ok(_) => {}
            Err(read_error) if read_error.kind() == std::io::ErrorKind::WouldBlock => return std::task::Poll::Pending,
            Err(read_error) if read_error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(read_error) => return std::task::Poll::Ready(Err(read_error)),
          }
        }
      }
      result => result,
    }
  }
}

fn tls_shutdown_result(result: std::io::Result<()>, peer_closed: bool) -> std::io::Result<()> {
  use std::io::ErrorKind;
  match result {
    Err(error)
      if peer_closed
        && matches!(
          error.kind(),
          ErrorKind::BrokenPipe | ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset
        ) =>
    {
      Ok(())
    }
    result => result,
  }
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn h2_tls_shutdown_authenticates_close_notify_after_a_failed_write() {
  use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
  use std::sync::Arc;
  use std::task::{Context, Poll, Waker};
  use tokio::io::AsyncWrite;

  struct DisconnectedWriter {
    input: std::io::Cursor<Vec<u8>>,
    eof: bool,
    error: std::io::ErrorKind,
  }
  impl Read for DisconnectedWriter {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
      if self.input.position() == self.input.get_ref().len() as u64 && !self.eof {
        return Err(std::io::ErrorKind::WouldBlock.into());
      }
      self.input.read(bytes)
    }
  }
  impl Write for DisconnectedWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
      Err(self.error.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
      Ok(())
    }
  }

  for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
    for error in [
      std::io::ErrorKind::BrokenPipe,
      std::io::ErrorKind::ConnectionAborted,
      std::io::ErrorKind::ConnectionReset,
    ] {
      for authenticated in [false, true] {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let cert = CertificateDer::from_pem_slice(include_bytes!("fixtures/localhost-cert.pem")).unwrap();
        let key = PrivateKeyDer::from_pem_slice(include_bytes!("fixtures/localhost-key.pem")).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.clone()).unwrap();
        let client_config = rustls::ClientConfig::builder_with_provider(provider.clone())
          .with_protocol_versions(&[version])
          .unwrap()
          .with_root_certificates(roots)
          .with_no_client_auth();
        let server_config = rustls::ServerConfig::builder_with_provider(provider)
          .with_protocol_versions(&[version])
          .unwrap()
          .with_no_client_auth()
          .with_single_cert(vec![cert], key)
          .unwrap();
        let mut client =
          rustls::ClientConnection::new(Arc::new(client_config), ServerName::try_from("localhost").unwrap()).unwrap();
        let mut server = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
        for _ in 0..10 {
          let mut wire = Vec::new();
          client.write_tls(&mut wire).unwrap();
          server.read_tls(&mut wire.as_slice()).unwrap();
          server.process_new_packets().unwrap();
          wire.clear();
          server.write_tls(&mut wire).unwrap();
          client.read_tls(&mut wire.as_slice()).unwrap();
          client.process_new_packets().unwrap();
          if !client.is_handshaking() && !server.is_handshaking() && !client.wants_write() && !server.wants_write() {
            break;
          }
        }
        assert!(!client.is_handshaking() && !server.is_handshaking());
        let socket = DisconnectedWriter {
          input: std::io::Cursor::new(Vec::new()),
          eof: false,
          error,
        };
        let mut io = H2TlsIo(rustls::StreamOwned::new(client, socket));
        let mut cx = Context::from_waker(Waker::noop());
        // The disconnect is not clean until the delayed TLS input authenticates it.
        assert!(std::pin::Pin::new(&mut io).poll_shutdown(&mut cx).is_pending());
        let mut wire = Vec::new();
        if authenticated {
          server.send_close_notify();
          server.write_tls(&mut wire).unwrap();
        }
        io.0.sock.input = std::io::Cursor::new(wire);
        io.0.sock.eof = true;
        match std::pin::Pin::new(&mut io).poll_shutdown(&mut cx) {
          Poll::Ready(result) if authenticated => result.unwrap(),
          Poll::Ready(result) => assert_eq!(result.unwrap_err().kind(), error),
          Poll::Pending => panic!("shutdown did not finish after TLS close or TCP EOF"),
        }
      }
    }
  }
}

#[test]
fn h2_tls_shutdown_requires_authenticated_close_before_accepting_disconnect() {
  use std::io::{Error, ErrorKind};
  for kind in [
    ErrorKind::BrokenPipe,
    ErrorKind::ConnectionAborted,
    ErrorKind::ConnectionReset,
  ] {
    assert!(tls_shutdown_result(Err(Error::from(kind)), true).is_ok());
    assert_eq!(
      tls_shutdown_result(Err(Error::from(kind)), false).unwrap_err().kind(),
      kind
    );
  }
  for peer_closed in [false, true] {
    for kind in [ErrorKind::InvalidData, ErrorKind::UnexpectedEof, ErrorKind::WouldBlock] {
      assert_eq!(
        tls_shutdown_result(Err(Error::from(kind)), peer_closed)
          .unwrap_err()
          .kind(),
        kind
      );
    }
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_h2_multiplexes_responses_over_verified_tls() {
  use rustls::pki_types::ServerName;
  use std::future::Future;
  use std::sync::Arc;
  use std::task::{Context, Poll, Waker};
  let (mut h, config) = tls_fixture_with_protocol(&[&rustls::version::TLS13], b"h2");
  let peer = h.peer.try_clone().unwrap();
  let (second_done, second_seen) = std::sync::mpsc::channel();
  let client = std::thread::spawn(move || {
    peer.set_nonblocking(true).unwrap();
    let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
    let io = H2TlsIo(rustls::StreamOwned::new(client, peer));
    let mut handshake = Box::pin(h2::client::Builder::new().handshake::<_, bytes::Bytes>(io));
    let mut cx = Context::from_waker(Waker::noop());
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut sender, connection) = loop {
      if let Poll::Ready(result) = handshake.as_mut().poll(&mut cx) {
        break result.unwrap();
      }
      assert!(Instant::now() < deadline, "H2 handshake timed out");
      std::thread::yield_now();
    };
    let request = |path| {
      http::Request::builder()
        .uri(format!("https://localhost/{path}"))
        .body(())
        .unwrap()
    };
    let (first, _) = sender.send_request(request("first"), true).unwrap();
    let (second, _) = sender.send_request(request("second"), true).unwrap();
    let mut replies = [Box::pin(first), Box::pin(second)];
    let mut bodies: [Option<h2::RecvStream>; 2] = [None, None];
    let mut result = [Vec::new(), Vec::new()];
    let mut ended = [false; 2];
    let mut connection = Box::pin(connection);
    let mut closed = false;
    while !closed || !ended.iter().all(|end| *end) {
      if !closed && let Poll::Ready(result) = connection.as_mut().poll(&mut cx) {
        result.unwrap();
        closed = true;
      }
      for index in 0..2 {
        if bodies[index].is_none()
          && let Poll::Ready(response) = replies[index].as_mut().poll(&mut cx)
        {
          let response = response.unwrap();
          assert_eq!(response.version(), http::Version::HTTP_2);
          bodies[index] = Some(response.into_body());
        }
        if !ended[index]
          && let Some(body) = &mut bodies[index]
        {
          loop {
            match body.poll_data(&mut cx) {
              Poll::Ready(Some(Ok(bytes))) => {
                result[index].extend_from_slice(&bytes);
                body.flow_control().release_capacity(bytes.len()).unwrap();
              }
              Poll::Ready(None) => {
                ended[index] = true;
                if index == 1 {
                  assert!(bodies[0].is_none());
                  second_done.send(()).unwrap();
                }
                break;
              }
              Poll::Ready(Some(Err(error))) => panic!("H2 response failed: {error}"),
              Poll::Pending => break,
            }
          }
        }
      }
      assert!(Instant::now() < deadline, "H2 responses timed out");
      std::thread::yield_now();
    }
    result
  });
  let requests = h.take(EVENT_REQUEST, 2);
  assert!(
    requests
      .iter()
      .all(|request| elide_transport_http_version(request.value) == 2)
  );
  let first = requests
    .iter()
    .find(|request| view(request.value, VIEW_PATH, 0) == b"/first")
    .unwrap()
    .value;
  let second = requests
    .iter()
    .find(|request| view(request.value, VIEW_PATH, 0) == b"/second")
    .unwrap()
    .value;
  respond(h.driver, second, 200, &[], b"second");
  assert_eq!(elide_transport_http_free(h.driver, second), 0);
  let deadline = Instant::now() + Duration::from_secs(10);
  while second_seen.try_recv().is_err() {
    assert!(Instant::now() < deadline);
    h.poll(10_000_000);
  }
  respond(h.driver, first, 200, &[], b"first");
  assert_eq!(elide_transport_http_free(h.driver, first), 0);
  elide_transport_http_drain(h.driver);
  h.take(EVENT_CLOSED, 1);
  let replies = client.join().unwrap();
  assert_eq!(replies[0], b"first");
  assert_eq!(replies[1], b"second");
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_tls_half_close_preserves_a_delayed_response() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  for (version, truncated) in [
    (&rustls::version::TLS12, false),
    (&rustls::version::TLS13, false),
    (&rustls::version::TLS13, true),
  ] {
    let (mut h, config) = tls_fixture_with_protocol(&[version], b"http/1.1");
    let peer = h.peer.try_clone().unwrap();
    let (closed, observed) = std::sync::mpsc::channel();
    let client = std::thread::spawn(move || {
      let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
      let mut stream = rustls::StreamOwned::new(client, peer);
      let request = if truncated {
        &b"POST /delayed HTTP/1.1\r\nContent-Length: 4\r\n\r\nab"[..]
      } else {
        &b"GET /delayed HTTP/1.1\r\nHost: localhost\r\n\r\n"[..]
      };
      stream.write_all(request).unwrap();
      stream.conn.send_close_notify();
      stream.flush().unwrap();
      closed.send(()).unwrap();
      let mut reply = Vec::new();
      stream.read_to_end(&mut reply).unwrap();
      reply
    });
    let request = h.take(EVENT_REQUEST, 1)[0].value;
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    // Drive the authenticated input close before settling the accepted handler.
    h.poll(100_000_000);
    h.poll(100_000_000);
    assert!(h.pending.iter().all(|event| event.kind != EVENT_CLOSED));
    if truncated {
      let (segments, end) = h.take_body(request);
      assert!(end.result < 0);
      assert_eq!(body_of(&segments), b"ab");
      h.release_segments(&segments);
    }
    assert_eq!(elide_transport_http_drain(h.driver), 1);
    respond(h.driver, request, 200, &[], b"delayed");
    assert_eq!(elide_transport_http_free(h.driver, request), 0);
    h.take(EVENT_CLOSED, 1);
    assert_eq!(responses(&client.join().unwrap())[0].1, b"delayed");
    assert_eq!(elide_transport_driver_release(h.driver), 0);
    assert_eq!(elide_transport_buffer_release(h.batch), 0);
    assert_eq!(elide_transport_owner_used(h.owner), 0);
    assert_eq!(elide_transport_owner_release(h.owner), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_h2_tls_half_close_preserves_a_delayed_response() {
  use rustls::pki_types::ServerName;
  use std::sync::Arc;
  let (mut h, config) = tls_fixture_with_protocol(&[&rustls::version::TLS13], b"h2");
  let peer = h.peer.try_clone().unwrap();
  let (closed, observed) = std::sync::mpsc::channel();
  let client = std::thread::spawn(move || {
    let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
    let mut stream = rustls::StreamOwned::new(client, peer);
    stream.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").unwrap();
    stream.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).unwrap();
    // Static HPACK entries GET, https, /; literal :authority localhost.
    let mut head = vec![0x82, 0x87, 0x84, 0x01, 9];
    head.extend_from_slice(b"localhost");
    stream.write_all(&[0, 0, head.len() as u8, 1, 5, 0, 0, 0, 1]).unwrap();
    stream.write_all(&head).unwrap();
    stream.conn.send_close_notify();
    stream.flush().unwrap();
    closed.send(()).unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    let mut body = Vec::new();
    let mut offset = 0;
    let mut ended = false;
    while offset < reply.len() {
      assert!(reply.len() - offset >= 9);
      let frame = &reply[offset..];
      let length = ((frame[0] as usize) << 16) | ((frame[1] as usize) << 8) | frame[2] as usize;
      assert!(frame.len() >= 9 + length);
      if frame[3] == 0 && frame[5..9] == [0, 0, 0, 1] {
        body.extend_from_slice(&frame[9..9 + length]);
        ended |= frame[4] & 1 != 0;
      }
      offset += 9 + length;
    }
    assert!(ended);
    body
  });
  let request = h.take(EVENT_REQUEST, 1)[0].value;
  observed.recv_timeout(Duration::from_secs(5)).unwrap();
  h.poll(100_000_000);
  h.poll(100_000_000);
  assert!(h.pending.iter().all(|event| event.kind != EVENT_CLOSED));
  respond(h.driver, request, 200, &[], b"delayed");
  assert_eq!(elide_transport_http_free(h.driver, request), 0);
  h.take(EVENT_CLOSED, 1);
  assert_eq!(client.join().unwrap(), b"delayed");
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn parsed_header_metadata_is_advertised_and_preserves_first_host() {
  for (head, length, host) in [
    ("GET / HTTP/1.0\r\n\r\n", -1i64, -1i32),
    (
      "GET / HTTP/1.1\r\nX-Test: yes\r\nHoSt: first\r\nHost: second\r\nContent-Length: 0\r\n\r\n",
      0,
      1,
    ),
    (
      "POST / HTTP/1.1\r\nHost: first\r\nContent-Length: 9223372036854775808\r\n\r\n",
      -1,
      0,
    ),
    (
      "POST / HTTP/1.1\r\nContent-Length: 4\r\nHost: first\r\nContent-Length: 4\r\n\r\n",
      4,
      1,
    ),
    (
      "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nHost: first\r\n\r\n",
      -1,
      1,
    ),
  ] {
    let mut h = harness();
    h.peer.write_all(head.as_bytes()).unwrap();
    let exchange = h.take(5, 1)[0].value;
    assert_ne!(
      flags(exchange) & 2,
      0,
      "extended metadata must be advertised before reading it"
    );
    let ptr = exchange as *const u8;
    // SAFETY: The live exchange ABI header contains the initialized i64 at offset 32.
    assert_eq!(unsafe { ptr.add(32).cast::<i64>().read_unaligned() }, length);
    // SAFETY: The live exchange ABI header contains the initialized i32 at offset 40.
    assert_eq!(unsafe { ptr.add(40).cast::<i32>().read_unaligned() }, host);
    if host >= 0 {
      assert_eq!(view(exchange, VIEW_HEADER_VALUE, host as u32), b"first");
    }
  }
}

/// Prepare the exchange's response buffer with `capacity` bytes and fill it with `wire`.
fn prepare(exchange: u64, capacity: usize, wire: &[u8]) {
  // SAFETY: The fixture holds the exchange lease; sentinel handles and invalid capacities are rejected.
  let address = unsafe { elide_transport_http_prepare(exchange, capacity as u64) };
  assert_ne!(address, 0, "prepare failed");
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(wire.as_ptr(), address as *mut u8, wire.len()) };
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn retained_heads_copy_spans_and_bytes_out_of_the_receive() {
  let mut h = harness();
  h.peer
    .write_all(b"GET /retain?q=1 HTTP/1.1\r\nHost: a\r\n\r\n")
    .unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  assert_eq!(
    // SAFETY: The exchange lease is held; null or misaligned sentinel arguments are rejected before access.
    unsafe { elide_transport_http_retain(exchange, std::ptr::null_mut()) },
    0
  );
  let mut length = 0u64;
  // SAFETY: The exchange lease is held; null or misaligned sentinel arguments are rejected before access.
  assert_eq!(unsafe { elide_transport_http_retain(0, &mut length) }, 0);
  // SAFETY: The exchange lease is held; null or misaligned sentinel arguments are rejected before access.
  assert_eq!(unsafe { elide_transport_http_retain(exchange + 1, &mut length) }, 0);
  // SAFETY: The exchange lease is held; null or misaligned sentinel arguments are rejected before access.
  let blob = unsafe { elide_transport_http_retain(exchange, &mut length) };
  assert_ne!(blob, 0);
  // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
  let bytes = unsafe { std::slice::from_raw_parts(blob as *const u8, length as usize) }.to_vec();
  let head = view(exchange, VIEW_HEAD, 0);
  let spans = u32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as usize;
  assert_eq!(spans, 8, "method, path and one header, as start/end pairs");
  assert_eq!(u32::from_ne_bytes(bytes[4..8].try_into().unwrap()) as usize, head.len());
  assert_eq!(&bytes[8..11], &[0, 1, 1], "GET, HTTP/1.1, keep-alive");
  assert_eq!(length as usize, 16 + spans * 4 + head.len());
  let table: Vec<u32> = bytes[16..16 + spans * 4]
    .chunks(4)
    .map(|chunk| u32::from_ne_bytes(chunk.try_into().unwrap()))
    .collect();
  let text = &bytes[16 + spans * 4..];
  assert_eq!(text, head.as_slice());
  let slice = |index: usize| &text[table[index] as usize..table[index + 1] as usize];
  assert_eq!(slice(0), b"GET");
  assert_eq!(slice(2), b"/retain?q=1");
  assert_eq!(slice(4), b"Host");
  assert_eq!(slice(6), b"a");
  // SAFETY: Consume the retained blob once; zero is rejected without dereferencing.
  assert_eq!(unsafe { elide_transport_http_head_release(blob) }, 0);
  // SAFETY: Consume the retained blob once; zero is rejected without dereferencing.
  assert_eq!(unsafe { elide_transport_http_head_release(0) }, INVALID);
  respond(h.driver, exchange, 204, &[], b"");
  let wire = h.read_while_polling(|wire| wire.ends_with(b"\r\n\r\n"));
  assert!(wire.starts_with(b"HTTP/1.1 204"));
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  // Keep-alive: the connection stays open until closed, and the driver refuses release until then.
  assert_eq!(elide_transport_socket_close(h.driver, h.socket), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn prepared_responses_are_sent_verbatim_and_can_close_the_connection() {
  let mut h = harness();
  h.peer
    .write_all(b"GET /a HTTP/1.1\r\nHost: a\r\n\r\nGET /b HTTP/1.1\r\nHost: a\r\n\r\n")
    .unwrap();
  let requests = h.take(EVENT_REQUEST, 2);
  let (first, second) = (requests[0].value, requests[1].value);
  // SAFETY: The fixture holds the exchange lease; sentinel handles and invalid capacities are rejected.
  assert_eq!(unsafe { elide_transport_http_prepare(0, 16) }, 0);
  // SAFETY: The fixture holds the exchange lease; sentinel handles and invalid capacities are rejected.
  assert_eq!(unsafe { elide_transport_http_prepare(first + 1, 16) }, 0);
  // SAFETY: The fixture holds the exchange lease; sentinel handles and invalid capacities are rejected.
  assert_eq!(unsafe { elide_transport_http_prepare(first, u64::MAX) }, 0);
  assert_eq!(
    elide_transport_http_send(h.driver, first, 4, 0),
    INVALID,
    "nothing prepared"
  );
  let one = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\none";
  prepare(first, 128, one);
  assert_eq!(elide_transport_http_send(0, first, one.len() as u64, 0), INVALID);
  assert_eq!(elide_transport_http_send(h.driver, 0, one.len() as u64, 0), INVALID);
  assert_eq!(elide_transport_http_send(h.driver, first, one.len() as u64, 0), 0);
  assert_eq!(
    elide_transport_http_send(h.driver, first, one.len() as u64, 0),
    INVALID,
    "responded"
  );
  let two = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\ntwo";
  prepare(second, two.len(), two);
  assert_eq!(
    elide_transport_http_send(h.driver, second, two.len() as u64, SEND_CLOSE),
    0
  );
  assert_eq!(h.take(EVENT_CLOSED, 1)[0].socket, h.socket);
  let mut got = Vec::new();
  h.peer.read_to_end(&mut got).unwrap();
  let mut expected = one.to_vec();
  expected.extend_from_slice(two);
  assert_eq!(got, expected);
  assert_eq!(elide_transport_http_free(h.driver, first), 0);
  assert_eq!(elide_transport_http_free(h.driver, second), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn an_oversized_send_keeps_the_prepared_response() {
  let mut h = harness();
  h.peer
    .write_all(b"GET / HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n")
    .unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;
  let wire = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";
  prepare(exchange, wire.len(), wire);
  assert_eq!(
    elide_transport_http_send(h.driver, exchange, wire.len() as u64 + 1, 0),
    INVALID
  );
  // A rejected send must leave the prepared buffer in place, like every other failed ABI call.
  assert_eq!(elide_transport_http_send(h.driver, exchange, wire.len() as u64, 0), 0);
  h.take(EVENT_CLOSED, 1);
  let mut got = Vec::new();
  h.peer.read_to_end(&mut got).unwrap();
  assert_eq!(got, wire);
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn http_calls_validate_arguments_before_touching_the_exchange() {
  use std::ptr::{null, null_mut};
  let mut h = harness();
  h.peer
    .write_all(b"GET /v HTTP/1.1\r\nHost: a\r\nX-Two: 2\r\n\r\n")
    .unwrap();
  let exchange = h.take(EVENT_REQUEST, 1)[0].value;

  let mut out = [7u64; 2];
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); output is writable or null for validation.
    unsafe { elide_transport_http_view(exchange, VIEW_PATH, 0, null_mut()) },
    INVALID
  );
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); output is writable or null for validation.
    unsafe { elide_transport_http_view(exchange, 99, 0, out.as_mut_ptr()) },
    INVALID
  );
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); output is writable or null for validation.
    unsafe { elide_transport_http_view(exchange, VIEW_HEADER_NAME, 2, out.as_mut_ptr()) },
    INVALID
  );
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); output is writable or null for validation.
    unsafe { elide_transport_http_view(0, VIEW_PATH, 0, out.as_mut_ptr()) },
    INVALID
  );
  assert_eq!(view(exchange, VIEW_HEADER_VALUE, 1), b"2");
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); output is writable or null for validation.
    unsafe { elide_transport_http_view(exchange, VIEW_BODY, 0, out.as_mut_ptr()) },
    0
  );
  assert_eq!(out, [0, 0], "HTTP/1 bodies are segments, never a view");

  let mut spans = [0u32; 12];
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); the declared output capacity fits the array.
    unsafe { elide_transport_http_spans(exchange, spans.as_mut_ptr(), 4) },
    12
  );
  assert_eq!(spans, [0; 12], "an undersized table is left untouched");
  // SAFETY: A null output requests the required span count without writing any memory.
  assert_eq!(unsafe { elide_transport_http_spans(exchange, null_mut(), 64) }, 12);
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); the declared output capacity fits the array.
    unsafe { elide_transport_http_spans(0, spans.as_mut_ptr(), 12) },
    INVALID
  );
  assert_eq!(
    // SAFETY: The exchange is live (or a rejected sentinel); the declared output capacity fits the array.
    unsafe { elide_transport_http_spans(exchange, spans.as_mut_ptr(), 12) },
    12
  );
  let head = view(exchange, VIEW_HEAD, 0);
  assert_eq!(&head[spans[2] as usize..spans[3] as usize], b"/v");
  assert_eq!(&head[spans[8] as usize..spans[9] as usize], b"X-Two");

  // SAFETY: The exchange lease and any header/body storage remain live; null invalid ranges are rejected.
  let respond_raw = |driver: u64, exchange: u64, status: u32, count: u32, length: u64| unsafe {
    elide_transport_http_respond(driver, exchange, status, null(), count, null(), length, 0)
  };
  assert_eq!(respond_raw(h.driver, exchange, 99, 0, 0), INVALID);
  assert_eq!(respond_raw(h.driver, exchange, 1000, 0, 0), INVALID);
  assert_eq!(
    respond_raw(h.driver, exchange, 200, 1, 0),
    INVALID,
    "headers are required"
  );
  assert_eq!(
    respond_raw(h.driver, exchange, 200, 0, 5),
    INVALID,
    "a body is required"
  );
  assert_eq!(respond_raw(0, exchange, 200, 0, 0), INVALID);
  assert_eq!(respond_raw(h.driver, 0, 200, 0, 0), INVALID);

  let mut address = 0u64;
  assert_eq!(
    // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
    unsafe { elide_transport_http_chunk_prepare(exchange, 8, null_mut()) },
    0
  );
  // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
  assert_eq!(unsafe { elide_transport_http_chunk_prepare(0, 8, &mut address) }, 0);
  assert_eq!(
    // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
    unsafe { elide_transport_http_chunk_prepare(exchange + 1, 8, &mut address) },
    0
  );
  assert_eq!(
    // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
    unsafe { elide_transport_http_chunk_prepare(exchange, u64::MAX, &mut address) },
    0
  );
  assert_eq!(
    // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
    unsafe { elide_transport_http_chunk_prepare(exchange, 1 << 40, &mut address) },
    0
  );
  assert_eq!(address, 0);
  // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
  let part = unsafe { elide_transport_http_chunk_prepare(exchange, 8, &mut address) };
  assert_ne!(part, 0);
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(b"streamed".as_ptr(), address as *mut u8, 8) };
  assert_eq!(
    elide_transport_http_chunk_send(h.driver, exchange, part, 8, 0),
    INVALID,
    "not streaming yet"
  );
  assert_eq!(elide_transport_http_chunk_send(0, exchange, part, 8, 0), INVALID);
  assert_eq!(elide_transport_http_chunk_send(h.driver, 0, part, 8, 0), INVALID);

  respond_stream(h.driver, exchange, 200, &[], u64::MAX);
  assert_eq!(respond_raw(h.driver, exchange, 200, 0, 0), INVALID, "already responded");
  let frozen = elide_transport_buffer_new(h.owner, 64);
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(frozen, 0) }, 0);
  assert_eq!(
    elide_transport_http_chunk_send(h.driver, exchange, frozen, 1, 0),
    INVALID
  );
  // A mutable handle smaller than the chunk framing is rejected, not underflowed.
  let small = elide_transport_buffer_new(h.owner, 8);
  assert_ne!(small, 0);
  for length in [0, 1] {
    assert_eq!(
      elide_transport_http_chunk_send(h.driver, exchange, small, length, 0),
      INVALID
    );
  }
  assert_eq!(
    elide_transport_http_chunk_send(h.driver, exchange, part, 9, 0),
    INVALID,
    "beyond capacity"
  );
  assert_eq!(
    elide_transport_http_chunk_send(h.driver, exchange, part, 8, CHUNK_FINAL),
    0
  );
  // SAFETY: The exchange lease is held; output is writable or null for validation; bad geometry is rejected.
  let late = unsafe { elide_transport_http_chunk_prepare(exchange, 1, &mut address) };
  assert_ne!(late, 0);
  assert_eq!(
    elide_transport_http_chunk_send(h.driver, exchange, late, 1, 0),
    INVALID,
    "response complete"
  );

  let got = h.read_while_polling(|wire| wire.ends_with(b"0\r\n\r\n"));
  let text = String::from_utf8_lossy(&got);
  assert!(text.contains("transfer-encoding: chunked\r\n"), "{text}");
  assert!(text.ends_with("\r\n\r\n8\r\nstreamed\r\n0\r\n\r\n"), "{text}");
  h.take(EVENT_PART_SENT, 1);
  for handle in [late, frozen, small] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  assert_eq!(elide_transport_socket_close(h.driver, h.socket), 0);
  h.finish();
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn native_h2_streams_request_bodies_and_rejects_unsupported_expectations_natively() {
  use rustls::pki_types::ServerName;
  use std::future::Future;
  use std::sync::Arc;
  use std::task::{Context, Poll, Waker};
  let (mut h, config) = tls_fixture_with_protocol(&[&rustls::version::TLS13], b"h2");
  let peer = h.peer.try_clone().unwrap();
  let client = std::thread::spawn(move || {
    peer.set_nonblocking(true).unwrap();
    let client = rustls::ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap()).unwrap();
    let io = H2TlsIo(rustls::StreamOwned::new(client, peer));
    let mut handshake = Box::pin(h2::client::Builder::new().handshake::<_, bytes::Bytes>(io));
    let mut cx = Context::from_waker(Waker::noop());
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut sender, connection) = loop {
      if let Poll::Ready(result) = handshake.as_mut().poll(&mut cx) {
        break result.unwrap();
      }
      assert!(Instant::now() < deadline, "H2 handshake timed out");
      std::thread::yield_now();
    };
    let upload = http::Request::builder()
      .method("POST")
      .uri("https://localhost/upload")
      .body(())
      .unwrap();
    let (upload, mut body) = sender.send_request(upload, false).unwrap();
    body.send_data(bytes::Bytes::from_static(b"hello h2"), true).unwrap();
    let rejected = http::Request::builder()
      .uri("https://localhost/expect")
      .header("expect", "fancy")
      .body(())
      .unwrap();
    let (rejected, _) = sender.send_request(rejected, true).unwrap();
    let mut replies = [Box::pin(upload), Box::pin(rejected)];
    let mut statuses = [0u16; 2];
    let mut bodies: [Option<h2::RecvStream>; 2] = [None, None];
    let mut result = [Vec::new(), Vec::new()];
    let mut ended = [false; 2];
    let mut connection = Box::pin(connection);
    let mut closed = false;
    while !closed || !ended.iter().all(|end| *end) {
      if !closed && let Poll::Ready(result) = connection.as_mut().poll(&mut cx) {
        result.unwrap();
        closed = true;
      }
      for index in 0..2 {
        if bodies[index].is_none()
          && let Poll::Ready(response) = replies[index].as_mut().poll(&mut cx)
        {
          let response = response.unwrap();
          statuses[index] = response.status().as_u16();
          bodies[index] = Some(response.into_body());
        }
        if !ended[index]
          && let Some(body) = &mut bodies[index]
        {
          loop {
            match body.poll_data(&mut cx) {
              Poll::Ready(Some(Ok(bytes))) => {
                result[index].extend_from_slice(&bytes);
                body.flow_control().release_capacity(bytes.len()).unwrap();
              }
              Poll::Ready(None) => {
                ended[index] = true;
                break;
              }
              Poll::Ready(Some(Err(error))) => panic!("H2 response failed: {error}"),
              Poll::Pending => break,
            }
          }
        }
      }
      assert!(Instant::now() < deadline, "H2 responses timed out");
      std::thread::yield_now();
    }
    (statuses, result)
  });
  // The rejected head is answered natively and never reaches the guest.
  let request = h.take(EVENT_REQUEST, 1);
  assert_eq!(request.len(), 1);
  let exchange = request[0].value;
  assert_eq!(elide_transport_http_version(exchange), 2);
  assert_eq!(view(exchange, VIEW_PATH, 0), b"/upload");
  assert_eq!(view(exchange, VIEW_METHOD, 0), b"POST");
  let (segments, end) = h.take_body(exchange);
  assert_eq!(end.result, 0);
  assert_eq!(body_of(&segments), b"hello h2");
  h.release_segments(&segments);
  respond(h.driver, exchange, 200, &[], b"stored");
  assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
  elide_transport_http_drain(h.driver);
  h.take(EVENT_CLOSED, 1);
  let (statuses, bodies) = client.join().unwrap();
  assert_eq!(statuses, [200, 417]);
  assert_eq!(bodies[0], b"stored");
  assert!(bodies[1].is_empty());
  assert!(h.pending.iter().all(|event| event.kind != EVENT_REQUEST));
  assert_eq!(elide_transport_driver_release(h.driver), 0);
  assert_eq!(elide_transport_buffer_release(h.batch), 0);
  assert_eq!(elide_transport_owner_used(h.owner), 0);
  assert_eq!(elide_transport_owner_release(h.owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "native sockets are unavailable under miri")]
fn retained_stream_parts_keep_handles_on_busy_invalid_and_length_failure() {
  for chunked in [false, true] {
    let mut h = harness();
    h.peer.write_all(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
    let exchange = h.take(EVENT_REQUEST, 1)[0].value;
    respond_stream(
      h.driver,
      exchange,
      200,
      &[],
      if chunked { u64::MAX } else { 300 * 1024 },
    );
    let body = elide_transport_buffer_new(h.owner, 200 * 1024);
    assert_ne!(body, 0);
    assert_eq!(
      elide_transport_http_chunk_send(h.driver, exchange, body, 1, CHUNK_RETAIN),
      INVALID
    );
    // SAFETY: buffer_new initializes all bytes, and no mutable lease remains.
    assert_eq!(unsafe { elide_transport_buffer_freeze(body, 200 * 1024) }, 0);
    if chunked {
      assert_eq!(
        elide_transport_http_chunk_send(h.driver, exchange, body, 1, CHUNK_RETAIN),
        INVALID
      );
      assert_eq!(elide_transport_buffer_release(body), 0);
      send_chunk(h.driver, exchange, b"", true);
      h.read_while_polling(|wire| wire.ends_with(b"0\r\n\r\n"));
    } else {
      assert_eq!(
        elide_transport_http_chunk_send(h.driver, exchange, body, 200 * 1024, CHUNK_RETAIN),
        0
      );
      assert_eq!(
        elide_transport_http_chunk_send(h.driver, exchange, body, 100 * 1024, CHUNK_RETAIN | CHUNK_FINAL),
        BUSY
      );
      h.read_while_polling(|wire| wire.len() > 200 * 1024);
      h.take(EVENT_PART_SENT, 2);
      // A short final part violates the declared length, but leaves the frozen handle owned.
      assert!(elide_transport_http_chunk_send(h.driver, exchange, body, 1, CHUNK_RETAIN | CHUNK_FINAL) < 0);
      assert_eq!(elide_transport_buffer_release(body), 0);
    }
    assert_eq!(elide_transport_http_free(h.driver, exchange), 0);
    if chunked {
      h.peer.shutdown(std::net::Shutdown::Both).unwrap();
    }
    h.take(EVENT_CLOSED, 1);
    h.finish();
  }
}
