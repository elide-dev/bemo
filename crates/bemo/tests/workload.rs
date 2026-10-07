mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use bemo::abi::*;

/// Every entry that starts work, with its leading `workload` parameter.
const WORK: &[&str] = &[
  "driver_new",
  "serving_worker_driver",
  "serving_driver",
  "serving_listen",
  "socket_listen",
  "socket_connect",
  "socket_accept",
  "socket_adopt",
  "socket_receive",
  "socket_receive_new",
  "socket_send",
  "driver_poll_callback",
  "driver_poll_batch_callback",
  "socket_http",
  "socket_http_tls",
  "tls_client",
  "tls_server",
  "tls_new",
];

fn frozen(owner: u64, bytes: &[u8]) -> u64 {
  let handle = elide_transport_buffer_new(owner, bytes.len() as u64);
  assert_ne!(handle, 0);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.address.cast(), bytes.len()) };
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(handle, bytes.len() as u64) }, 0);
  handle
}

fn endpoint(owner: u64, address: SocketAddr) -> u64 {
  let SocketAddr::V4(v4) = address else {
    panic!("ipv4 endpoint expected")
  };
  let mut bytes = [0u8; 24];
  bytes[..4].copy_from_slice(&v4.ip().octets());
  bytes[16..18].copy_from_slice(&address.port().to_ne_bytes());
  bytes[18..20].copy_from_slice(&4u16.to_ne_bytes());
  frozen(owner, &bytes)
}

#[derive(Clone, Copy, Debug)]
struct Completion {
  operation: u64,
  socket: u64,
  value: u64,
  result: i64,
  kind: u32,
}

/// Poll until `accept` matches a completion, returning it; wall-clock bounded.
fn wait(driver: u64, batch: u64, accept: impl Fn(&Completion) -> bool) -> Completion {
  poll_until(driver, batch, 50_000_000, accept)
}

fn poll_until(driver: u64, batch: u64, timeout_ns: u64, accept: impl Fn(&Completion) -> bool) -> Completion {
  let deadline = Instant::now() + Duration::from_secs(5);
  while Instant::now() < deadline {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, timeout_ns, batch, 8) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
    let events = unsafe { std::slice::from_raw_parts(view.address.cast::<NativeEvent>(), count as usize) };
    for event in events {
      let event = Completion {
        operation: event.operation,
        socket: event.socket,
        value: event.value,
        result: event.result,
        kind: event.kind,
      };
      if accept(&event) {
        return event;
      }
      if event.kind == 3 && event.value != 0 {
        elide_transport_buffer_release(event.value);
      }
    }
  }
  panic!("no matching completion before the deadline");
}

fn connect(workload: u64, driver: u64, batch: u64, listener: &TcpListener) -> (u64, TcpStream) {
  let address = endpoint(workload, listener.local_addr().unwrap());
  let socket = elide_transport_socket_connect(workload, driver, address);
  assert_ne!(socket, 0);
  assert_eq!(elide_transport_buffer_release(address), 0);
  let peer = std::thread::scope(|scope| {
    let peer = scope.spawn(|| listener.accept().unwrap().0);
    let connected = wait(driver, batch, |event| event.kind == 1 && event.socket == socket);
    assert_eq!(connected.result, 0);
    peer.join().unwrap()
  });
  (socket, peer)
}

fn release(driver: u64) {
  let deadline = Instant::now() + Duration::from_secs(5);
  while elide_transport_driver_release(driver) == BUSY {
    assert!(Instant::now() < deadline, "driver release stayed busy");
  }
}

#[test]
fn header_declares_the_workload_convention_and_version() {
  let header = include_str!("../../../include/elide_transport.h");
  assert!(header.contains(&format!(
    "#define ELIDE_TRANSPORT_ABI_VERSION {}",
    elide_transport_abi_version()
  )));
  assert_eq!(elide_transport_abi_version(), 3);
  let declarations: Vec<(&str, &str)> = header
    .lines()
    .filter_map(|line| line.split_once('('))
    .filter_map(|(head, rest)| {
      let name = head.split_whitespace().last()?.strip_prefix("elide_transport_")?;
      Some((name, rest))
    })
    .collect();
  for name in WORK {
    let (_, parameters) = declarations
      .iter()
      .find(|(declared, _)| declared == name)
      .unwrap_or_else(|| panic!("{name} is not declared"));
    assert!(
      parameters.starts_with("uint64_t workload"),
      "{name} must take a leading uint64_t workload"
    );
  }
  assert!(
    declarations
      .iter()
      .any(|(name, parameters)| *name == "workload_close" && parameters.starts_with("uint64_t workload)"))
  );
}

#[test]
fn unknown_and_released_workloads_start_nothing() {
  let backend = common::backend() as u32;
  assert_eq!(elide_transport_driver_new(0, backend, 4), 0);
  assert_eq!(elide_transport_workload_close(0), INVALID);
  let workload = elide_transport_owner_new(64);
  assert_eq!(elide_transport_owner_release(workload), 0);
  assert_eq!(elide_transport_workload_close(workload), INVALID);
  assert_eq!(elide_transport_driver_new(workload, backend, 4), 0);
  assert_eq!(elide_transport_tls_client(workload, 0, 0), 0);
  assert_eq!(elide_transport_tls_server(workload, 0, 0, 0), 0);
  assert_eq!(elide_transport_tls_new(workload, 0, workload, 0), 0);
}

#[test]
#[cfg_attr(miri, ignore = "opens sockets and a driver")]
fn conflicting_workloads_share_a_driver_and_close_independently() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let host = elide_transport_owner_new(4096);
  let small = elide_transport_owner_new(64);
  let large = elide_transport_owner_new(1024 * 1024);
  let driver = elide_transport_driver_new(host, common::backend() as u32, 16);
  assert_ne!(driver, 0);
  let batch = elide_transport_buffer_new(host, 8 * size_of::<NativeEvent>() as u64);
  let (a, _peer_a) = connect(small, driver, batch, &listener);
  let (b, mut peer_b) = connect(large, driver, batch, &listener);

  // Each workload admits work only on its own sockets, within its own limit.
  assert_eq!(elide_transport_socket_receive_new(large, driver, a, large, 32), 0);
  assert_eq!(elide_transport_socket_receive_new(small, driver, b, small, 32), 0);
  assert_eq!(elide_transport_socket_receive_new(small, driver, a, small, 4096), 0);
  let pending = elide_transport_socket_receive_new(small, driver, a, small, 32);
  assert_ne!(pending, 0);
  assert_eq!(elide_transport_owner_used(small), 32);
  let receive = elide_transport_socket_receive_new(large, driver, b, large, 4096);
  assert_ne!(receive, 0);

  assert_eq!(elide_transport_workload_close(small), 0);
  assert_eq!(elide_transport_workload_close(small), 0, "closing is idempotent");
  let cancelled = wait(driver, batch, |event| event.operation == pending);
  assert_eq!((cancelled.kind, cancelled.socket, cancelled.value), (3, a, 0));
  assert!(cancelled.result < 0, "cancelled receive reported {}", cancelled.result);
  let payload = frozen(large, b"late");
  assert_eq!(elide_transport_socket_send(small, driver, a, payload, 0, 4), 0);
  assert_eq!(elide_transport_socket_receive_new(small, driver, a, small, 16), 0);
  let address = endpoint(large, listener.local_addr().unwrap());
  assert_eq!(elide_transport_socket_connect(small, driver, address), 0);
  assert_eq!(elide_transport_driver_new(small, common::backend() as u32, 4), 0);
  unsafe extern "C" fn untouched(_: *mut std::ffi::c_void, _: *const NativeEvent, _: u32) -> i32 {
    0
  }
  let poll =
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    |workload| unsafe { elide_transport_driver_poll_batch_callback(workload, driver, 0, 8, Some(untouched), std::ptr::null_mut()) };
  assert_eq!(poll(small), INVALID, "closed callback workload");
  assert!(poll(large) >= 0, "open callback workload");

  // The other workload keeps serving on the same driver.
  peer_b.write_all(b"ping").unwrap();
  let received = wait(driver, batch, |event| event.operation == receive);
  assert_eq!((received.kind, received.socket, received.result), (3, b, 4));
  assert_eq!(elide_transport_buffer_release(received.value), 0);
  let send = elide_transport_socket_send(large, driver, b, payload, 0, 4);
  assert_ne!(send, 0);
  let sent = wait(driver, batch, |event| event.operation == send);
  assert_eq!((sent.kind, sent.result), (4, 4));
  let mut echoed = [0u8; 4];
  peer_b.read_exact(&mut echoed).unwrap();
  assert_eq!(&echoed, b"late");

  assert_eq!(elide_transport_socket_close(driver, a), 0);
  assert_eq!(elide_transport_socket_close(driver, b), 0);
  for handle in [payload, address, batch] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  release(driver);
  assert_eq!(elide_transport_owner_used(small), 0);
  assert_eq!(elide_transport_owner_used(large), 0);
  for owner in [small, large, host] {
    assert_eq!(elide_transport_owner_release(owner), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "opens sockets and a driver")]
fn closing_from_another_thread_cancels_at_the_owner_poll() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let workload = elide_transport_owner_new(1024 * 1024);
  let (ready, armed) = std::sync::mpsc::channel();
  let owner = std::thread::spawn(move || {
    let driver = elide_transport_driver_new(workload, common::backend() as u32, 8);
    let batch = elide_transport_buffer_new(workload, 8 * size_of::<NativeEvent>() as u64);
    let (socket, peer) = connect(workload, driver, batch, &listener);
    let pending = elide_transport_socket_receive_new(workload, driver, socket, workload, 64);
    assert_ne!(pending, 0);
    ready.send(()).unwrap();
    // Only the closing thread's wakeup can end these polls before the deadline.
    let cancelled = poll_until(driver, batch, 60_000_000_000, |event| event.operation == pending);
    assert!(cancelled.result < 0);
    assert_eq!(elide_transport_socket_close(driver, socket), 0);
    assert_eq!(elide_transport_buffer_release(batch), 0);
    release(driver);
    drop(peer);
  });
  armed.recv().unwrap();
  assert_eq!(elide_transport_workload_close(workload), 0);
  owner.join().unwrap();
  assert_eq!(elide_transport_owner_used(workload), 0);
  assert_eq!(elide_transport_owner_release(workload), 0);
}

#[test]
#[cfg_attr(miri, ignore = "opens sockets and a driver")]
fn accepted_sockets_keep_their_listener_workload() {
  let workload = elide_transport_owner_new(1024 * 1024);
  let other = elide_transport_owner_new(1024);
  let driver = elide_transport_driver_new(workload, common::backend() as u32, 8);
  let batch = elide_transport_buffer_new(workload, 8 * size_of::<NativeEvent>() as u64);
  let address = endpoint(workload, "127.0.0.1:0".parse().unwrap());
  assert_eq!(elide_transport_socket_listen(other, 0, address, 16, 0), 0);
  let listener = elide_transport_socket_listen(workload, driver, address, 16, 0);
  assert_ne!(listener, 0);
  let local = elide_transport_buffer_new(workload, 24);
  assert_eq!(elide_transport_socket_address(driver, listener, 0, local), 0);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(local, &mut view) }, 0);
  // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
  let bytes = unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), 24) };
  let port = u16::from_ne_bytes([bytes[16], bytes[17]]);

  assert_eq!(elide_transport_socket_accept(other, driver, listener), 0);
  let accept = elide_transport_socket_accept(workload, driver, listener);
  assert_ne!(accept, 0);
  let _client = TcpStream::connect(("127.0.0.1", port)).unwrap();
  let accepted = wait(driver, batch, |event| event.operation == accept);
  assert_eq!((accepted.kind, accepted.result), (2, 0));
  assert_eq!(elide_transport_socket_adopt(other, driver, accepted.value), INVALID);
  assert_eq!(elide_transport_socket_adopt(workload, driver, accepted.value), 0);
  let pending = elide_transport_socket_receive_new(workload, driver, accepted.value, workload, 64);
  assert_ne!(pending, 0);

  // Releasing closes the workload; on the owner thread the sweep runs immediately.
  assert_eq!(elide_transport_owner_release(workload), 0);
  assert_eq!(elide_transport_socket_accept(workload, driver, listener), 0);
  let cancelled = wait(driver, batch, |event| event.operation == pending);
  assert!(cancelled.result < 0);
  assert_eq!(elide_transport_socket_close(driver, accepted.value), 0);
  assert_eq!(elide_transport_socket_close(driver, listener), 0);
  for handle in [address, local, batch] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  release(driver);
  assert_eq!(elide_transport_owner_release(other), 0);
}

#[test]
#[cfg_attr(miri, ignore = "opens sockets and a driver")]
fn closing_a_workload_retires_its_http_sockets() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let workload = elide_transport_owner_new(1024 * 1024);
  let other = elide_transport_owner_new(1024 * 1024);
  let driver = elide_transport_driver_new(other, common::backend() as u32, 8);
  let batch = elide_transport_buffer_new(other, 8 * size_of::<NativeEvent>() as u64);
  let (socket, _peer) = connect(workload, driver, batch, &listener);
  assert_eq!(
    elide_transport_socket_http(other, driver, socket, other, 16 * 1024),
    INVALID
  );
  assert_eq!(
    elide_transport_socket_http(workload, driver, socket, workload, 16 * 1024),
    0
  );
  assert_eq!(elide_transport_workload_close(workload), 0);
  let closed = wait(driver, batch, |event| {
    event.kind == EVENT_CLOSED && event.socket == socket
  });
  assert_eq!(closed.result, 0);
  assert_eq!(elide_transport_socket_close(driver, socket), INVALID);
  assert_eq!(elide_transport_buffer_release(batch), 0);
  release(driver);
  assert_eq!(elide_transport_owner_used(workload), 0);
  assert_eq!(elide_transport_owner_release(workload), 0);
  assert_eq!(elide_transport_owner_release(other), 0);
}

#[test]
#[cfg_attr(miri, ignore = "rustls configuration calls into AWS-LC")]
fn tls_sessions_of_a_closed_workload_only_shut_down() {
  let workload = elide_transport_owner_new(1024 * 1024);
  let chain = frozen(workload, include_bytes!("fixtures/localhost-cert.pem"));
  let key = frozen(workload, include_bytes!("fixtures/localhost-key.pem"));
  let context = elide_transport_tls_server(workload, chain, key, 0);
  assert_ne!(context, 0);
  let session = elide_transport_tls_new(workload, context, workload, 0);
  assert_ne!(session, 0);
  let output = elide_transport_buffer_new(workload, 32);
  let plaintext = frozen(workload, b"data");
  let input = elide_transport_buffer_new(workload, 16);
  assert_eq!(elide_transport_workload_close(workload), 0);
  assert_eq!(elide_transport_tls_new(workload, context, workload, 0), 0);
  assert_eq!(elide_transport_tls_server(workload, chain, key, 0), 0);
  assert_eq!(elide_transport_tls_feed(session, input, 1), INVALID);
  assert_eq!(elide_transport_tls_step(session, 2, plaintext, 0, 4, output), INVALID);
  assert_ne!(elide_transport_tls_step(session, 3, 0, 0, 0, output), INVALID);
  assert_eq!(elide_transport_tls_release(session), 0);
  assert_eq!(elide_transport_tls_context_release(context), 0);
  for handle in [chain, key, output, plaintext, input] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_used(workload), 0);
  assert_eq!(elide_transport_owner_release(workload), 0);
}
