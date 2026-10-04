mod common;

use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use dokar::abi::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};

const CERT: &[u8] = include_bytes!("fixtures/localhost-cert.pem");
const KEY: &[u8] = include_bytes!("fixtures/localhost-key.pem");
const H2: &[u8] = b"\x02h2";

fn frozen(owner: u64, bytes: &[u8]) -> u64 {
  let handle = elide_transport_buffer_new(owner, bytes.len() as u64);
  assert_ne!(handle, 0);
  let mut view = BufferView::default();
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.address.cast::<u8>(), bytes.len()) };
  assert_eq!(unsafe { elide_transport_buffer_freeze(handle, bytes.len() as u64) }, 0);
  handle
}

fn contents(handle: u64) -> Vec<u8> {
  let mut view = BufferView::default();
  assert_eq!(unsafe { elide_transport_buffer_view(handle, &mut view) }, 0);
  unsafe { std::slice::from_raw_parts(view.address.cast::<u8>(), view.length as usize) }.to_vec()
}

fn fields(descriptor: u64) -> [u64; 4] {
  let bytes = contents(descriptor);
  assert_eq!(bytes.len(), 32);
  std::array::from_fn(|index| u64::from_ne_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap()))
}

fn completion(driver: u64, batch: u64, kind: u32) -> (u64, i64) {
  for _ in 0..64 {
    let count = unsafe { elide_transport_driver_poll(driver, 1_000_000_000, batch, 8) };
    assert!(count >= 0, "poll failed");
    let mut view = BufferView::default();
    assert_eq!(unsafe { elide_transport_buffer_view(batch, &mut view) }, 0);
    for index in 0..count as usize {
      let event = unsafe { &*view.address.cast::<NativeEvent>().add(index) };
      if event.kind == kind {
        return (event.value, event.result);
      }
    }
  }
  panic!("no completion of kind {kind}");
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

fn peer_client() -> rustls::Connection {
  let mut roots = rustls::RootCertStore::empty();
  roots.add(CertificateDer::from_pem_slice(CERT).unwrap()).unwrap();
  let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
  let mut config = rustls::ClientConfig::builder_with_provider(provider)
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
  config.alpn_protocols = vec![b"h2".to_vec()];
  let name = ServerName::try_from("localhost").unwrap();
  rustls::ClientConnection::new(Arc::new(config), name).unwrap().into()
}

fn peer_server() -> rustls::Connection {
  let cert = CertificateDer::from_pem_slice(CERT).unwrap();
  let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
  let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
  let mut config = rustls::ServerConfig::builder_with_provider(provider)
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
  config.alpn_protocols = vec![b"h2".to_vec()];
  rustls::ServerConnection::new(Arc::new(config)).unwrap().into()
}

/// A driver-owned socket connected to a std stream; TLS bytes from the peer arrive as driver
/// receives, which are the only mutable buffers `tls_feed` accepts.
struct Link {
  owner: u64,
  driver: u64,
  batch: u64,
  socket: u64,
  stream: TcpStream,
  in_flight: usize,
}

impl Link {
  fn new() -> Self {
    let owner = elide_transport_owner_new(8 * 1024 * 1024);
    let driver = elide_transport_driver_new(common::workload(), common::backend() as u32, 16);
    assert_ne!(driver, 0);
    let batch = elide_transport_buffer_new(owner, 8 * size_of::<NativeEvent>() as u64);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = endpoint(owner, listener.local_addr().unwrap());
    let socket = elide_transport_socket_connect(common::workload(), driver, target);
    assert_ne!(socket, 0);
    let accepted = std::thread::spawn(move || listener.accept().unwrap().0);
    assert_eq!(completion(driver, batch, 1).1, 0);
    let stream = accepted.join().unwrap();
    assert_eq!(elide_transport_buffer_release(target), 0);
    Self {
      owner,
      driver,
      batch,
      socket,
      stream,
      in_flight: 0,
    }
  }

  fn send_raw(&mut self, bytes: &[u8]) {
    self.stream.write_all(bytes).unwrap();
    self.in_flight += bytes.len();
  }

  /// Receive everything the peer wrote and feed it to `session` before any step.
  fn feed_all(&mut self, session: u64, capacity: u64) {
    while self.in_flight > 0 {
      assert_ne!(
        elide_transport_socket_receive_new(common::workload(), self.driver, self.socket, self.owner, capacity),
        0
      );
      let (input, received) = completion(self.driver, self.batch, 3);
      assert!(received > 0, "receive failed: {received}");
      self.in_flight -= received as usize;
      assert_eq!(elide_transport_tls_feed(session, input, received as u64), 0);
    }
  }

  fn finish(self) {
    assert_eq!(elide_transport_socket_close(self.driver, self.socket), 0);
    assert_eq!(elide_transport_buffer_release(self.batch), 0);
    assert_eq!(elide_transport_driver_release(self.driver), 0);
    assert_eq!(elide_transport_owner_used(self.owner), 0, "TLS storage leaked");
    assert_eq!(elide_transport_owner_release(self.owner), 0);
  }
}

struct Outcome {
  abi_received: Vec<u8>,
  peer_received: Vec<u8>,
  authenticated: bool,
  final_state: u64,
}

/// Exchange one message each way, then close from the ABI side and wait for the peer's close.
fn converse(link: &mut Link, session: u64, mut peer: rustls::Connection, capacity: u64) -> Outcome {
  let descriptor = elide_transport_buffer_new(link.owner, 32);
  let message = frozen(link.owner, b"from-abi");
  peer.writer().write_all(b"from-peer").unwrap();
  let mut abi_received = Vec::new();
  let mut peer_received = Vec::new();
  let (mut ack, mut sent, mut close_sent, mut peer_close_sent) = (false, false, false, false);
  let mut authenticated = false;
  let mut final_state = None;
  for _ in 0..500 {
    while peer.wants_write() {
      let mut wire = Vec::new();
      peer.write_tls(&mut wire).unwrap();
      link.send_raw(&wire);
    }
    link.feed_all(session, capacity);
    let exchanged = abi_received == b"from-peer" && peer_received == b"from-abi";
    let action = if ack {
      1
    } else if !sent {
      2
    } else if exchanged && !close_sent {
      3
    } else {
      0
    };
    assert_eq!(elide_transport_tls_step(session, action, message, 0, 8, descriptor), 0);
    let [state, accepted, encoded, plaintext] = fields(descriptor);
    authenticated |= (state >> 32) & 1 == 1;
    let state = state & 0xffff_ffff;
    if accepted != 0 {
      assert_eq!(accepted, 8);
      sent = true;
    }
    if action == 3 && state == 3 {
      close_sent = true;
    }
    ack = encoded != 0 || state == 2;
    if encoded != 0 {
      let bytes = contents(encoded);
      assert_eq!(elide_transport_buffer_release(encoded), 0);
      let mut cursor = Cursor::new(bytes.as_slice());
      while (cursor.position() as usize) < bytes.len() {
        peer.read_tls(&mut cursor).unwrap();
        let io = peer.process_new_packets().unwrap();
        if io.peer_has_closed() && !peer_close_sent {
          peer.send_close_notify();
          peer_close_sent = true;
        }
      }
    }
    if plaintext != 0 {
      abi_received.extend(contents(plaintext));
      assert_eq!(elide_transport_buffer_release(plaintext), 0);
    }
    let mut chunk = [0u8; 64];
    while let Ok(count @ 1..) = peer.reader().read(&mut chunk) {
      peer_received.extend_from_slice(&chunk[..count]);
    }
    if state == 5 || state == 6 {
      final_state = Some(state);
      break;
    }
  }
  assert!(peer_close_sent, "peer never observed close_notify");
  assert_eq!(elide_transport_buffer_release(message), 0);
  assert_eq!(elide_transport_buffer_release(descriptor), 0);
  Outcome {
    abi_received,
    peer_received,
    authenticated,
    final_state: final_state.expect("ABI session never observed the peer close"),
  }
}

fn negotiated_protocol(owner: u64, session: u64) -> Vec<u8> {
  let output = elide_transport_buffer_new(owner, 16);
  let length = elide_transport_tls_protocol(session, output);
  assert!(length >= 0);
  let protocol = contents(output)[..length as usize].to_vec();
  assert_eq!(elide_transport_buffer_release(output), 0);
  protocol
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC; real sockets")]
fn client_session_handshakes_exchanges_and_closes_through_the_abi() {
  let mut link = Link::new();
  let roots = frozen(link.owner, CERT);
  let alpn = frozen(link.owner, H2);
  let context = elide_transport_tls_client(common::workload(), roots, alpn);
  assert_ne!(context, 0);
  let name = frozen(link.owner, b"localhost");
  let session = elide_transport_tls_new(common::workload(), context, link.owner, name);
  assert_ne!(session, 0);
  // Sessions keep their configuration after the context is released.
  assert_eq!(elide_transport_tls_context_release(context), 0);
  assert_eq!(elide_transport_tls_context_release(context), INVALID);
  assert_eq!(
    elide_transport_tls_new(common::workload(), context, link.owner, name),
    0
  );

  let outcome = converse(&mut link, session, peer_server(), 16 * 1024);
  assert_eq!(outcome.abi_received, b"from-peer");
  assert_eq!(outcome.peer_received, b"from-abi");
  assert!(outcome.authenticated);
  assert!(matches!(outcome.final_state, 5 | 6));
  assert_eq!(negotiated_protocol(link.owner, session), b"h2");

  assert_eq!(elide_transport_tls_release(session), 0);
  assert_eq!(elide_transport_tls_release(session), INVALID);
  for handle in [roots, alpn, name] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  link.finish();
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC; real sockets")]
fn server_session_reassembles_small_receives_before_stepping() {
  let mut link = Link::new();
  let chain = frozen(link.owner, CERT);
  let key = frozen(link.owner, KEY);
  let alpn = frozen(link.owner, H2);
  let context = elide_transport_tls_server(common::workload(), chain, key, alpn);
  assert_ne!(context, 0);
  let session = elide_transport_tls_new(common::workload(), context, link.owner, 0);
  assert_ne!(session, 0);
  // 16-byte receives force every ClientHello fragment through the growth path of `tls_feed`.
  let outcome = converse(&mut link, session, peer_client(), 16);
  assert_eq!(outcome.abi_received, b"from-peer");
  assert_eq!(outcome.peer_received, b"from-abi");
  assert!(matches!(outcome.final_state, 5 | 6));
  assert_eq!(negotiated_protocol(link.owner, session), b"h2");
  assert_eq!(elide_transport_tls_release(session), 0);
  assert_eq!(elide_transport_tls_context_release(context), 0);
  for handle in [chain, key, alpn] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  link.finish();
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC; real sockets")]
fn malformed_records_fail_the_session_terminally() {
  let mut link = Link::new();
  let chain = frozen(link.owner, CERT);
  let key = frozen(link.owner, KEY);
  let context = elide_transport_tls_server(common::workload(), chain, key, 0);
  assert_ne!(context, 0);
  let session = elide_transport_tls_new(common::workload(), context, link.owner, 0);
  assert_ne!(session, 0);
  let descriptor = elide_transport_buffer_new(link.owner, 32);
  link.send_raw(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
  link.feed_all(session, 4096);
  assert_eq!(elide_transport_tls_step(session, 0, 0, 0, 0, descriptor), -3);
  assert_eq!(elide_transport_tls_step(session, 0, 0, 0, 0, descriptor), -3);
  // The descriptor is returned to the caller on failure.
  assert_eq!(elide_transport_buffer_release(descriptor), 0);
  assert_eq!(negotiated_protocol(link.owner, session), b"");
  assert_eq!(elide_transport_tls_release(session), 0);
  assert_eq!(elide_transport_tls_context_release(context), 0);
  for handle in [chain, key] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  link.finish();
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn session_creation_and_feed_validate_their_arguments() {
  let owner = elide_transport_owner_new(1024 * 1024);
  let roots = frozen(owner, CERT);
  let context = elide_transport_tls_client(common::workload(), roots, 0);
  assert_ne!(context, 0);
  assert_eq!(
    elide_transport_tls_new(common::workload(), context, owner, 0),
    0,
    "clients need a name"
  );
  let invalid_utf8 = frozen(owner, b"\xff\xfe");
  assert_eq!(
    elide_transport_tls_new(common::workload(), context, owner, invalid_utf8),
    0
  );
  let invalid_name = frozen(owner, b"not a host name");
  assert_eq!(
    elide_transport_tls_new(common::workload(), context, owner, invalid_name),
    0
  );
  let name = frozen(owner, b"localhost");
  assert_eq!(
    elide_transport_tls_new(common::workload(), context, 0, name),
    0,
    "sessions need an owner"
  );
  let session = elide_transport_tls_new(common::workload(), context, owner, name);
  assert_ne!(session, 0);
  assert_eq!(negotiated_protocol(owner, session), b"");

  let fresh = elide_transport_buffer_new(owner, 16);
  assert_eq!(elide_transport_tls_feed(session, fresh, u64::MAX), INVALID);
  assert_eq!(
    elide_transport_tls_feed(session, fresh, 1),
    INVALID,
    "no initialized bytes to transfer"
  );
  assert_eq!(
    elide_transport_tls_feed(session, name, 1),
    INVALID,
    "frozen input is rejected"
  );
  assert_eq!(elide_transport_tls_feed(session, 0, 1), INVALID);
  assert_eq!(elide_transport_tls_protocol(session, name), INVALID);
  assert_eq!(elide_transport_tls_protocol(session, 0), INVALID);

  let descriptor = elide_transport_buffer_new(owner, 32);
  assert_eq!(elide_transport_tls_step(session, 0, 0, 0, 0, descriptor), 0);
  let [state, _, encoded, plaintext] = fields(descriptor);
  assert_eq!(state & 0xffff_ffff, 1, "a fresh client encodes its ClientHello");
  assert_eq!(state >> 32, 0, "no peer is authenticated yet");
  assert_ne!(encoded, 0);
  assert_eq!(plaintext, 0);
  assert!(contents(encoded).len() > 5);
  assert_eq!(elide_transport_buffer_release(encoded), 0);

  assert_eq!(elide_transport_tls_release(session), 0);
  assert_eq!(elide_transport_tls_context_release(context), 0);
  for handle in [roots, invalid_utf8, invalid_name, name, fresh, descriptor] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "server contexts load their key through AWS-LC")]
fn server_context_rejects_an_empty_chain() {
  let owner = elide_transport_owner_new(64 * 1024);
  let empty_chain = frozen(owner, b"no certificates here\n");
  let key = frozen(owner, KEY);
  assert_eq!(elide_transport_tls_server(common::workload(), empty_chain, key, 0), 0);
  for handle in [empty_chain, key] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "certificate parsing is slow under miri")]
fn contexts_reject_malformed_pem_and_alpn_before_building() {
  let owner = elide_transport_owner_new(64 * 1024);
  let roots = frozen(owner, CERT);
  let not_pem = frozen(owner, b"hello");
  let bad_base64 = frozen(owner, b"-----BEGIN CERTIFICATE-----\n@@@@\n-----END CERTIFICATE-----\n");
  let not_der = frozen(owner, b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n");
  let zero_length = frozen(owner, b"\x00");
  let truncated = frozen(owner, b"\x05h2");
  let key = frozen(owner, KEY);

  assert_eq!(elide_transport_tls_client(common::workload(), 0, 0), 0);
  assert_eq!(
    elide_transport_tls_client(common::workload(), not_pem, 0),
    0,
    "no trust anchors"
  );
  assert_eq!(elide_transport_tls_client(common::workload(), bad_base64, 0), 0);
  assert_eq!(elide_transport_tls_client(common::workload(), not_der, 0), 0);
  assert_eq!(elide_transport_tls_client(common::workload(), roots, zero_length), 0);
  assert_eq!(elide_transport_tls_client(common::workload(), roots, truncated), 0);
  assert_eq!(
    elide_transport_tls_client(common::workload(), roots, not_pem),
    0,
    "'h' is not a valid ALPN length prefix"
  );

  assert_eq!(elide_transport_tls_server(common::workload(), 0, key, 0), 0);
  assert_eq!(elide_transport_tls_server(common::workload(), roots, 0, 0), 0);
  assert_eq!(elide_transport_tls_server(common::workload(), roots, key, truncated), 0);
  assert_eq!(elide_transport_tls_server(common::workload(), bad_base64, key, 0), 0);
  assert_eq!(
    elide_transport_tls_server(common::workload(), roots, not_pem, 0),
    0,
    "no private key"
  );

  for handle in [roots, not_pem, bad_base64, not_der, zero_length, truncated, key] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
fn session_calls_reject_unknown_handles_and_malformed_steps() {
  let owner = elide_transport_owner_new(4096);
  let descriptor = elide_transport_buffer_new(owner, 32);
  let small = elide_transport_buffer_new(owner, 8);
  let plaintext = frozen(owner, b"data");

  assert_eq!(elide_transport_tls_context_release(0), INVALID);
  assert_eq!(elide_transport_tls_new(common::workload(), 0, owner, 0), 0);
  assert_eq!(elide_transport_tls_feed(0, small, 0), INVALID);
  assert_eq!(elide_transport_tls_step(0, 4, 0, 0, 0, descriptor), INVALID);
  assert_eq!(elide_transport_tls_step(0, 2, 0, 0, 1, descriptor), INVALID);
  assert_eq!(
    elide_transport_tls_step(0, 2, plaintext, u64::MAX, 1, descriptor),
    INVALID
  );
  assert_eq!(elide_transport_tls_step(0, 2, plaintext, 2, 3, descriptor), INVALID);
  assert_eq!(elide_transport_tls_step(0, 0, 0, 0, 0, 0), INVALID);
  assert_eq!(elide_transport_tls_step(0, 0, 0, 0, 0, plaintext), INVALID);
  assert_eq!(elide_transport_tls_step(0, 0, 0, 0, 0, small), INVALID);
  assert_eq!(elide_transport_tls_step(0, 2, plaintext, 0, 4, descriptor), INVALID);
  assert_eq!(elide_transport_tls_step(0, 1, 0, 0, 0, descriptor), INVALID);
  assert_eq!(elide_transport_tls_protocol(0, descriptor), INVALID);
  assert_eq!(elide_transport_tls_release(0), INVALID);

  // Descriptors survive a rejected step and stay owned by the caller.
  for handle in [descriptor, small, plaintext] {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
}

#[test]
#[cfg_attr(miri, ignore = "TLS ABI uses sockets and AWS-LC")]
fn tls_contexts_require_their_open_workload_for_sessions_and_http() {
  let first = elide_transport_owner_new(8 * 1024 * 1024);
  let mut link = Link::new();
  let chain = frozen(first, CERT);
  let key = frozen(first, KEY);
  let alpn = frozen(first, H2);
  let name = frozen(first, b"localhost");
  let client = elide_transport_tls_client(first, chain, alpn);
  let server = elide_transport_tls_server(first, chain, key, alpn);
  let sibling = elide_transport_tls_server(common::workload(), chain, key, alpn);
  for context in [client, server, sibling] {
    assert_ne!(context, 0);
  }
  assert_eq!(elide_transport_tls_new(common::workload(), client, link.owner, name), 0);
  assert_eq!(elide_transport_tls_new(common::workload(), server, link.owner, 0), 0);
  assert_eq!(
    elide_transport_socket_http_tls(common::workload(), link.driver, link.socket, link.owner, 4096, server),
    INVALID
  );
  let existing = elide_transport_tls_new(first, server, first, 0);
  assert_ne!(existing, 0);
  assert_eq!(elide_transport_workload_close(first), 0);
  assert_eq!(elide_transport_tls_new(first, client, link.owner, name), 0);
  assert_eq!(elide_transport_tls_new(first, server, link.owner, 0), 0);
  assert_eq!(elide_transport_tls_client(first, chain, alpn), 0);
  assert_eq!(elide_transport_tls_server(first, chain, key, alpn), 0);
  assert_eq!(
    elide_transport_socket_http_tls(common::workload(), link.driver, link.socket, link.owner, 4096, server),
    INVALID
  );
  assert_eq!(elide_transport_tls_release(existing), 0);

  // Rejected HTTP activation leaves the sibling's socket usable with its own TLS context.
  let session = elide_transport_tls_new(common::workload(), sibling, link.owner, 0);
  assert_ne!(session, 0);
  for context in [client, server, sibling] {
    assert_eq!(elide_transport_tls_context_release(context), 0);
  }
  let outcome = converse(&mut link, session, peer_client(), 4096);
  assert!(outcome.authenticated);
  assert_eq!(outcome.abi_received, b"from-peer");
  assert_eq!(outcome.peer_received, b"from-abi");
  assert_eq!(elide_transport_tls_release(session), 0);
  link.finish();
  for buffer in [chain, key, alpn, name] {
    assert_eq!(elide_transport_buffer_release(buffer), 0);
  }
  assert_eq!(elide_transport_owner_used(first), 0);
  assert_eq!(elide_transport_owner_release(first), 0);
}
