mod common;

use std::io::{Read, Write};
use std::net::{Ipv6Addr, SocketAddr, TcpListener, TcpStream};

use bemo::abi::*;

fn endpoint_bytes(address: SocketAddr) -> [u8; 24] {
  let mut bytes = [0u8; 24];
  match address {
    SocketAddr::V4(v4) => {
      bytes[..4].copy_from_slice(&v4.ip().octets());
      bytes[18..20].copy_from_slice(&4u16.to_ne_bytes());
    }
    SocketAddr::V6(v6) => {
      bytes[..16].copy_from_slice(&v6.ip().octets());
      bytes[18..20].copy_from_slice(&6u16.to_ne_bytes());
      bytes[20..24].copy_from_slice(&v6.scope_id().to_ne_bytes());
    }
  }
  bytes[16..18].copy_from_slice(&address.port().to_ne_bytes());
  bytes
}

fn frozen(owner: u64, bytes: &[u8]) -> u64 {
  let handle = elide_transport_buffer_new(owner, bytes.len() as u64);
  assert_ne!(handle, 0);
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  // SAFETY: The fixture owns the destination capacity; the source is a separate live byte slice.
  unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.address.cast::<u8>(), bytes.len()) };
  // SAFETY: The fixture has no live writers; allocation initializes capacity and oversized lengths are rejected.
  assert_eq!(unsafe { elide_transport_buffer_freeze(handle, bytes.len() as u64) }, 0);
  handle
}

fn endpoint(owner: u64, address: SocketAddr) -> u64 {
  frozen(owner, &endpoint_bytes(address))
}

fn contents(handle: u64) -> Vec<u8> {
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  // SAFETY: The retained handle owns this initialized byte range for the duration of the copy/read.
  unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), view.length as usize) }.to_vec()
}

fn decode(bytes: &[u8]) -> SocketAddr {
  assert_eq!(bytes.len(), 24);
  let port = u16::from_ne_bytes([bytes[16], bytes[17]]);
  match u16::from_ne_bytes([bytes[18], bytes[19]]) {
    4 => SocketAddr::from(([bytes[0], bytes[1], bytes[2], bytes[3]], port)),
    6 => {
      let ip: [u8; 16] = bytes[..16].try_into().unwrap();
      SocketAddr::from((Ipv6Addr::from(ip), port))
    }
    family => panic!("unexpected family {family}"),
  }
}

fn local_address(driver: u64, owner: u64, socket: u64, peer: u32) -> SocketAddr {
  let output = elide_transport_buffer_new(owner, 24);
  assert_eq!(elide_transport_socket_address(driver, socket, peer, output), 0);
  let address = decode(&contents(output));
  assert_eq!(elide_transport_buffer_release(output), 0);
  address
}

fn completion(driver: u64, batch: u64, kind: u32) -> (u64, i64) {
  for _ in 0..64 {
    // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
    let count = unsafe { elide_transport_driver_poll(driver, 1_000_000_000, batch, 8) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    for index in 0..count as usize {
      // SAFETY: poll initialized this event range; the live batch allocation is aligned for NativeEvent.
      let event = unsafe { &*view.address.cast::<NativeEvent>().add(index) };
      if event.kind == kind {
        return (event.value, event.result);
      }
    }
  }
  panic!("no completion of kind {kind}");
}

struct Fixture {
  owner: u64,
  driver: u64,
  batch: u64,
}

impl Fixture {
  fn new() -> Self {
    let owner = elide_transport_owner_new(4 * 1024 * 1024);
    assert_ne!(owner, 0);
    let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 16);
    assert_ne!(driver, 0);
    let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
    assert_ne!(batch, 0);
    Self { owner, driver, batch }
  }

  fn listen(&self, address: SocketAddr) -> (u64, SocketAddr) {
    let endpoint = endpoint(self.owner, address);
    let listener = elide_transport_socket_listen(common::workload(), self.driver, endpoint, 16, 1);
    assert_ne!(listener, 0, "listen failed: {}", elide_transport_last_error());
    assert_eq!(elide_transport_last_error(), 0);
    assert_eq!(elide_transport_buffer_release(endpoint), 0);
    let bound = local_address(self.driver, self.owner, listener, 0);
    (listener, bound)
  }

  fn accept(&self, listener: u64) -> u64 {
    assert_ne!(
      elide_transport_socket_accept(common::workload(), self.driver, listener),
      0
    );
    let (accepted, result) = completion(self.driver, self.batch, 2);
    assert_eq!(result, 0);
    assert_ne!(accepted, 0);
    accepted
  }
}

impl Drop for Fixture {
  fn drop(&mut self) {
    if std::thread::panicking() {
      return;
    }
    assert_eq!(elide_transport_buffer_release(self.batch), 0);
    assert_eq!(elide_transport_driver_release(self.driver), 0);
    assert_eq!(elide_transport_owner_release(self.owner), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn listener_accept_adopt_send_receive_and_shutdown() {
  let f = Fixture::new();
  let (listener, bound) = f.listen("127.0.0.1:0".parse().unwrap());
  assert!(bound.is_ipv4());
  assert_ne!(bound.port(), 0);
  let scratch = elide_transport_buffer_new(f.owner, 24);
  assert_eq!(elide_transport_socket_address(f.driver, listener, 1, scratch), INVALID);
  assert_eq!(elide_transport_socket_address(f.driver, listener, 2, scratch), INVALID);
  assert_eq!(elide_transport_socket_address(f.driver, 0, 0, scratch), INVALID);
  assert_eq!(elide_transport_socket_address(0, listener, 0, scratch), INVALID);
  assert_eq!(elide_transport_buffer_release(scratch), 0);

  assert_ne!(elide_transport_socket_accept(common::workload(), f.driver, listener), 0);
  assert_eq!(
    elide_transport_socket_accept(common::workload(), f.driver, listener),
    0,
    "a second accept must wait for the first to complete"
  );
  let mut client = TcpStream::connect(bound).unwrap();
  let (accepted, result) = completion(f.driver, f.batch, 2);
  assert_eq!(result, 0);
  assert_ne!(accepted, 0);
  assert_eq!(elide_transport_socket_adopt(common::workload(), 0, accepted), INVALID);
  assert_eq!(elide_transport_socket_adopt(common::workload(), f.driver, accepted), 0);
  assert_eq!(
    elide_transport_socket_adopt(common::workload(), f.driver, accepted),
    INVALID
  );
  assert_eq!(elide_transport_socket_discard(accepted), INVALID);

  let peer = local_address(f.driver, f.owner, accepted, 1);
  assert_eq!(peer, client.local_addr().unwrap());
  assert_eq!(local_address(f.driver, f.owner, accepted, 0), bound);

  for (option, value) in [(1, 1), (1, 0), (2, 1), (3, 65536), (4, 65536), (5, 1)] {
    assert_eq!(
      elide_transport_socket_option(f.driver, accepted, option, value),
      0,
      "option {option}={value}"
    );
  }
  for (option, value) in [(0, 1), (1, 2), (3, 0), (4, -1), (99, 1)] {
    assert_eq!(
      elide_transport_socket_option(f.driver, accepted, option, value),
      -3,
      "option {option}={value} must be unsupported"
    );
  }
  assert_eq!(elide_transport_socket_option(0, accepted, 1, 1), INVALID);
  assert_eq!(elide_transport_socket_option(f.driver, 0, 1, 1), INVALID);

  let payload = frozen(f.owner, b"hello world");
  assert_eq!(
    elide_transport_socket_send(common::workload(), f.driver, accepted, payload, 6, 100),
    0
  );
  assert_eq!(
    elide_transport_socket_send(common::workload(), f.driver, accepted, payload, u64::MAX, 2),
    0
  );
  assert_eq!(
    elide_transport_socket_send(common::workload(), 0, accepted, payload, 0, 5),
    0
  );
  assert_eq!(
    elide_transport_socket_send(common::workload(), f.driver, 0, payload, 0, 5),
    0
  );
  let mutable = elide_transport_buffer_new(f.owner, 8);
  assert_eq!(
    elide_transport_socket_send(common::workload(), f.driver, accepted, mutable, 0, 1),
    0
  );
  assert_ne!(
    elide_transport_socket_send(common::workload(), f.driver, accepted, payload, 6, 5),
    0
  );
  assert_eq!(completion(f.driver, f.batch, 4).1, 5);
  let mut wire = [0u8; 5];
  client.read_exact(&mut wire).unwrap();
  assert_eq!(&wire, b"world");
  assert_eq!(elide_transport_buffer_release(payload), 0);

  // A caller-owned receive buffer is unavailable until its completion republishes it.
  assert_eq!(
    elide_transport_socket_receive(common::workload(), f.driver, accepted, payload),
    0
  );
  assert_eq!(
    elide_transport_socket_receive(common::workload(), 0, accepted, mutable),
    0
  );
  assert_eq!(
    elide_transport_socket_receive(common::workload(), f.driver, 0, mutable),
    0
  );
  assert_ne!(
    elide_transport_socket_receive(common::workload(), f.driver, accepted, mutable),
    0
  );
  assert_eq!(
    elide_transport_socket_receive(common::workload(), f.driver, accepted, mutable),
    0
  );
  let mut view = BufferView::default();
  // SAFETY: The output points to a writable BufferView; handle validation occurs before buffer access.
  assert_eq!(unsafe { elide_transport_buffer_view(mutable, &mut view) }, INVALID);
  client.write_all(b"ping").unwrap();
  let (buffer, received) = completion(f.driver, f.batch, 3);
  assert_eq!(buffer, mutable);
  assert_eq!(received, 4);
  assert_eq!(&contents(mutable)[..4], b"ping");

  assert_eq!(elide_transport_socket_shutdown(f.driver, accepted, 3), INVALID);
  assert_eq!(elide_transport_socket_shutdown(0, accepted, 1), INVALID);
  assert_eq!(elide_transport_socket_shutdown(f.driver, 0, 1), INVALID);
  assert_eq!(elide_transport_socket_shutdown(f.driver, accepted, 1), 0);
  let mut eof = [0u8; 1];
  assert_eq!(
    client.read(&mut eof).unwrap(),
    0,
    "write shutdown must reach the peer as EOF"
  );

  assert_eq!(elide_transport_socket_close(0, accepted), INVALID);
  assert_eq!(elide_transport_socket_close(f.driver, accepted), 0);
  assert_eq!(elide_transport_socket_close(f.driver, accepted), INVALID);
  assert_eq!(elide_transport_socket_close(f.driver, listener), 0);
  assert_eq!(elide_transport_buffer_release(mutable), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn accepted_sockets_can_be_discarded_before_adoption() {
  let f = Fixture::new();
  let (listener, bound) = f.listen("127.0.0.1:0".parse().unwrap());
  let _client = TcpStream::connect(bound).unwrap();
  let accepted = f.accept(listener);
  assert_eq!(elide_transport_socket_discard(accepted), 0);
  assert_eq!(elide_transport_socket_discard(accepted), INVALID);
  assert_eq!(
    elide_transport_socket_adopt(common::workload(), f.driver, accepted),
    INVALID
  );
  assert_eq!(elide_transport_socket_accept(common::workload(), f.driver, 0), 0);
  assert_eq!(elide_transport_socket_accept(common::workload(), 0, listener), 0);
  assert_eq!(elide_transport_socket_close(f.driver, listener), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn connect_reports_success_and_refusal_as_events() {
  let f = Fixture::new();
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let target = endpoint(f.owner, listener.local_addr().unwrap());
  let socket = elide_transport_socket_connect(common::workload(), f.driver, target);
  assert_ne!(socket, 0);
  assert_eq!(elide_transport_last_error(), 0);
  let accepted = std::thread::spawn(move || listener.accept().unwrap().0);
  assert_eq!(completion(f.driver, f.batch, 1).1, 0);
  let mut peer = accepted.join().unwrap();
  let local = local_address(f.driver, f.owner, socket, 0);
  assert_eq!(local, peer.peer_addr().unwrap());

  assert_ne!(
    elide_transport_socket_receive_new(common::workload(), f.driver, socket, f.owner, 64),
    0
  );
  peer.write_all(b"abc").unwrap();
  let (buffer, received) = completion(f.driver, f.batch, 3);
  assert_eq!(received, 3);
  assert_eq!(contents(buffer), b"abc");
  assert_eq!(elide_transport_buffer_release(buffer), 0);
  assert_eq!(
    elide_transport_socket_receive_new(common::workload(), f.driver, socket, 0, 64),
    0
  );
  assert_eq!(
    elide_transport_socket_receive_new(common::workload(), f.driver, socket, f.owner, 0),
    0
  );
  assert_eq!(
    elide_transport_socket_receive_new(common::workload(), f.driver, 0, f.owner, 64),
    0
  );
  assert_eq!(
    elide_transport_socket_receive_new(common::workload(), 0, socket, f.owner, 64),
    0
  );

  // Peer EOF completes with zero bytes and publishes no buffer.
  assert_ne!(
    elide_transport_socket_receive_new(common::workload(), f.driver, socket, f.owner, 64),
    0
  );
  drop(peer);
  let (buffer, received) = completion(f.driver, f.batch, 3);
  assert_eq!((buffer, received), (0, 0));
  assert_eq!(elide_transport_socket_close(f.driver, socket), 0);

  let closed = {
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    probe.local_addr().unwrap()
  };
  let refused = endpoint(f.owner, closed);
  let socket = elide_transport_socket_connect(common::workload(), f.driver, refused);
  assert_ne!(socket, 0);
  assert_eq!(completion(f.driver, f.batch, 1).1, -4, "ECONNREFUSED maps to -4");
  assert_eq!(elide_transport_socket_close(f.driver, socket), 0);
  assert_eq!(elide_transport_buffer_release(target), 0);
  assert_eq!(elide_transport_buffer_release(refused), 0);
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn malformed_endpoints_and_listen_failures_set_last_error() {
  let f = Fixture::new();
  let loopback: SocketAddr = "127.0.0.1:0".parse().unwrap();

  let mutable = elide_transport_buffer_new(f.owner, 24);
  assert_eq!(elide_transport_socket_connect(common::workload(), f.driver, mutable), 0);
  assert_eq!(elide_transport_last_error(), INVALID);
  assert_eq!(elide_transport_last_error(), 0, "reading the error clears it");
  assert_eq!(
    elide_transport_socket_listen(common::workload(), f.driver, mutable, 16, 1),
    0
  );
  assert_eq!(elide_transport_last_error(), INVALID);

  let short = frozen(f.owner, &endpoint_bytes(loopback)[..23]);
  assert_eq!(elide_transport_socket_connect(common::workload(), f.driver, short), 0);
  let mut bytes = endpoint_bytes(loopback);
  bytes[18..20].copy_from_slice(&5u16.to_ne_bytes());
  let family = frozen(f.owner, &bytes);
  assert_eq!(elide_transport_socket_connect(common::workload(), f.driver, family), 0);
  assert_eq!(elide_transport_last_error(), INVALID);

  let valid = endpoint(f.owner, loopback);
  assert_eq!(elide_transport_socket_connect(common::workload(), 0, valid), 0);
  assert_eq!(elide_transport_socket_listen(common::workload(), 0, valid, 16, 1), 0);
  assert_eq!(
    elide_transport_socket_listen(common::workload(), f.driver, valid, 16, 2),
    0
  );
  assert_eq!(
    elide_transport_socket_listen(common::workload(), f.driver, valid, -1, 1),
    0
  );
  assert_eq!(elide_transport_last_error(), -3, "negative backlog is invalid input");

  let occupied = TcpListener::bind(loopback).unwrap();
  let taken = endpoint(f.owner, occupied.local_addr().unwrap());
  assert_eq!(
    elide_transport_socket_listen(common::workload(), f.driver, taken, 16, 0),
    0
  );
  assert_eq!(elide_transport_last_error(), -9, "EADDRINUSE maps to -9");

  for handle in [mutable, short, family, valid, taken] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "io_uring and real sockets are unavailable under miri")]
fn ipv6_endpoints_round_trip_through_listen_and_address() {
  if TcpListener::bind("[::1]:0").is_err() {
    return;
  }
  let f = Fixture::new();
  let (listener, bound) = f.listen("[::1]:0".parse().unwrap());
  assert_eq!(bound.ip(), Ipv6Addr::LOCALHOST);
  let mut client = TcpStream::connect(bound).unwrap();
  let accepted = f.accept(listener);
  assert_eq!(elide_transport_socket_adopt(common::workload(), f.driver, accepted), 0);
  assert_eq!(
    local_address(f.driver, f.owner, accepted, 1),
    client.local_addr().unwrap()
  );
  client.write_all(b"v6").unwrap();
  assert_ne!(
    elide_transport_socket_receive_new(common::workload(), f.driver, accepted, f.owner, 16),
    0
  );
  let (buffer, received) = completion(f.driver, f.batch, 3);
  assert_eq!(received, 2);
  assert_eq!(contents(buffer), b"v6");
  assert_eq!(elide_transport_buffer_release(buffer), 0);
  assert_eq!(elide_transport_socket_close(f.driver, accepted), 0);
  assert_eq!(elide_transport_socket_close(f.driver, listener), 0);
}

#[test]
fn polls_reject_invalid_batches_drivers_and_callbacks() {
  let owner = elide_transport_owner_new(4096);
  let small = elide_transport_buffer_new(owner, 8);
  let batch = elide_transport_buffer_new(owner, size_of::<NativeEvent>() as u64);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(0, 0, 0, 1) }, INVALID);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(0, 0, small, 1) }, INVALID);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(0, 0, batch, 0) }, INVALID);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(0, 0, batch, 2) }, INVALID);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(0, 0, batch, u32::MAX) }, INVALID);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(0, 0, batch, 1) }, INVALID);
  // The batch is returned to its owner even when the driver is unknown.
  assert_eq!(elide_transport_buffer_release(batch), 0);
  let frozen_batch = frozen(owner, &[0u8; 40]);
  // SAFETY: The fixture owns the driver and batch with space for the requested number of events.
  assert_eq!(unsafe { elide_transport_driver_poll(0, 0, frozen_batch, 1) }, INVALID);

  unsafe extern "C" fn never(_: u64, _: *const NativeEvent) -> i32 {
    unreachable!("no events are dispatched for an unknown driver")
  }
  assert_eq!(
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    unsafe { elide_transport_driver_poll_callback(common::workload(), 0, 0, 1, None, 0) },
    INVALID
  );
  assert_eq!(
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    unsafe { elide_transport_driver_poll_callback(common::workload(), 0, 0, 0, Some(never), 0) },
    INVALID
  );
  assert_eq!(
    // SAFETY: The callback and its stack context stay live throughout synchronous polling on the owner thread.
    unsafe { elide_transport_driver_poll_callback(common::workload(), 0, 0, 1, Some(never), 0) },
    INVALID
  );
  assert_eq!(elide_transport_socket_discard(0), INVALID);
  assert_eq!(elide_transport_last_error(), 0);

  assert_eq!(elide_transport_buffer_release(small), 0);
  assert_eq!(elide_transport_buffer_release(frozen_batch), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg(unix)]
#[cfg_attr(miri, ignore = "native driver and Unix sockets are unavailable under miri")]
fn unix_connect_exchanges_bytes_and_rejects_malformed_paths() {
  use std::os::unix::{ffi::OsStrExt, net::UnixListener};
  let f = Fixture::new();
  let directory = std::env::temp_dir().join(format!("v2-{:x}-{:x}", std::process::id(), f.owner));
  std::fs::create_dir(&directory).unwrap();
  struct Cleanup(std::path::PathBuf);
  impl Drop for Cleanup {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.0);
    }
  }
  let _cleanup = Cleanup(directory.clone());
  let path = directory.join("s");
  let listener = UnixListener::bind(&path).unwrap();
  let mut bytes = vec![0; 24];
  bytes[18..20].copy_from_slice(&1u16.to_ne_bytes());
  bytes.extend_from_slice(path.as_os_str().as_bytes());
  let target = frozen(f.owner, &bytes);
  let socket = elide_transport_socket_connect(common::workload(), f.driver, target);
  assert_ne!(socket, 0);
  assert_eq!(elide_transport_buffer_release(target), 0);
  assert_eq!(completion(f.driver, f.batch, 1).1, 0);
  let (mut peer, _) = listener.accept().unwrap();
  peer.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
  let body = frozen(f.owner, b"unix request");
  assert_ne!(
    elide_transport_socket_send(common::workload(), f.driver, socket, body, 0, 12),
    0
  );
  assert_eq!(completion(f.driver, f.batch, 4).1, 12);
  assert_eq!(elide_transport_buffer_release(body), 0);
  let mut received = [0; 12];
  peer.read_exact(&mut received).unwrap();
  assert_eq!(&received, b"unix request");
  assert_ne!(
    elide_transport_socket_receive_new(common::workload(), f.driver, socket, f.owner, 64),
    0
  );
  peer.write_all(b"reply").unwrap();
  let (buffer, count) = completion(f.driver, f.batch, 3);
  assert_eq!(count, 5);
  assert_eq!(contents(buffer), b"reply");
  assert_eq!(elide_transport_buffer_release(buffer), 0);
  assert_eq!(elide_transport_socket_close(f.driver, socket), 0);
  let mut nul = bytes.clone();
  nul.push(0);
  let mut oversized = bytes[..24].to_vec();
  oversized.extend_from_slice(&[b'x'; 256]);
  let mut reserved = bytes.clone();
  reserved[20] = 1;
  for malformed in [bytes[..24].to_vec(), nul, oversized, reserved] {
    let target = frozen(f.owner, &malformed);
    assert_eq!(elide_transport_socket_connect(common::workload(), f.driver, target), 0);
    assert_eq!(elide_transport_last_error(), INVALID);
    assert_eq!(elide_transport_buffer_release(target), 0);
  }
}
