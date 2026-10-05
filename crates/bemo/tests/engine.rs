use bemo::abi::engine::*;
use bemo::abi::{
  INVALID, elide_transport_owner_new, elide_transport_owner_release, elide_transport_owner_used,
  elide_transport_workload_close,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};

const CERT: &[u8] = include_bytes!("fixtures/localhost-cert.pem");
const KEY: &[u8] = include_bytes!("fixtures/localhost-key.pem");
const CLIENT_CERT: &[u8] = include_bytes!("fixtures/client-cert.pem");
const CLIENT_KEY: &[u8] = include_bytes!("fixtures/client-key.pem");
const ALPN: &[u8] = b"\x02h2\x08http/1.1";
const CLOSED: i64 = 3;
const FINISHED: i64 = 1;

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn client_identity_is_separate_from_server_trust_and_required_by_peer() {
  use std::sync::Arc;
  let owner = Owner::new();
  let identity = [CLIENT_CERT, b"\n", CLIENT_KEY].concat();
  for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
    for mode in ["absent", "pem", "selected", "unmatched", "multiple", "empty-hints"] {
      let present = !matches!(mode, "absent" | "unmatched");
      let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
      let mut roots = rustls::RootCertStore::empty();
      roots.add(CertificateDer::from_pem_slice(CLIENT_CERT).unwrap()).unwrap();
      let issuer = roots.subjects()[0].as_ref().to_vec();
      let selected = identity_list(&[(CLIENT_CERT, CLIENT_KEY, &issuer)]);
      let unrelated = identity_list(&[(CLIENT_CERT, CLIENT_KEY, b"unrelated")]);
      let multiple = identity_list(&[(CERT, KEY, b"unrelated"), (CLIENT_CERT, CLIENT_KEY, &issuer)]);
      let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone());
      let verifier = if mode == "empty-hints" {
        verifier.clear_root_hint_subjects()
      } else {
        verifier
      };
      let verifier = verifier.build().unwrap();
      let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[version])
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
          vec![CertificateDer::from_pem_slice(CERT).unwrap()],
          PrivateKeyDer::from_pem_slice(KEY).unwrap(),
        )
        .unwrap();
      let mut server = rustls::ServerConnection::new(Arc::new(config)).unwrap();
      let handle = context(
        owner.0,
        match mode {
          "absent" => 0,
          "pem" => ENGINE_CLIENT_IDENTITY,
          _ => ENGINE_CLIENT_IDENTITIES,
        },
        CERT,
        match mode {
          "absent" => &[],
          "pem" => &identity,
          "unmatched" => &unrelated,
          "multiple" => &multiple,
          _ => &selected,
        },
      );
      assert_ne!(handle, 0);
      let client = engine(owner.0, handle, Some("localhost"));
      assert_ne!(client, 0);
      let mut established = false;
      for _ in 0..32 {
        let mut wire = Vec::new();
        wrap(client, &[], &mut wire);
        server.read_tls(&mut wire.as_slice()).unwrap();
        if server.process_new_packets().is_err() {
          break;
        }
        wire.clear();
        server.write_tls(&mut wire).unwrap();
        if unwrap(client, &mut wire, &mut Vec::new(), false).is_err() {
          break;
        }
        if !server.is_handshaking() && !Packed(elide_transport_engine_control(client, 0)).flag(55) {
          established = true;
          break;
        }
      }
      assert_eq!(
        established, present,
        "client authentication for {:?}, {mode}",
        version.version
      );
      if present {
        assert_eq!(
          server.peer_certificates().unwrap()[0],
          CertificateDer::from_pem_slice(CLIENT_CERT).unwrap()
        );
      }
      assert_eq!(
        // SAFETY: The fixture owns the engine; output is a live writable slice or null with zero capacity.
        unsafe { elide_transport_engine_info(client, 7, 0, std::ptr::null_mut(), 0) },
        i32::from(present)
      );
      if present {
        let expected = CertificateDer::from_pem_slice(CLIENT_CERT).unwrap();
        let mut local = vec![0; expected.len()];
        assert_eq!(
          // SAFETY: The fixture owns the engine; output is a live writable slice or null with zero capacity.
          unsafe { elide_transport_engine_info(client, 8, 0, local.as_mut_ptr(), local.len() as u64) },
          expected.len() as i32
        );
        assert_eq!(local, expected.as_ref());
      }
      let fresh = engine(owner.0, handle, Some("localhost"));
      assert_ne!(fresh, 0);
      assert_eq!(
        // SAFETY: The fixture owns the engine; output is a live writable slice or null with zero capacity.
        unsafe { elide_transport_engine_info(fresh, 7, 0, std::ptr::null_mut(), 0) },
        0
      );
      assert_eq!(elide_transport_engine_release(fresh), 0);
      assert_eq!(elide_transport_engine_release(client), 0);
      assert_eq!(elide_transport_engine_context_release(handle), 0);
      assert_eq!(elide_transport_owner_used(owner.0), 0);
    }
  }
  for (flags, input) in [
    (0, identity.as_slice()),
    (ENGINE_CLIENT_IDENTITY, &[][..]),
    (ENGINE_CLIENT_IDENTITY, CLIENT_CERT),
    (ENGINE_CLIENT_IDENTITY, CLIENT_KEY),
    (ENGINE_CLIENT_IDENTITY | ENGINE_SERVER, identity.as_slice()),
  ] {
    assert_eq!(context(owner.0, flags, CERT, input), 0);
  }
  let mismatched = [CLIENT_CERT, b"\n", KEY].concat();
  assert_eq!(context(owner.0, ENGINE_CLIENT_IDENTITY, CERT, &mismatched), 0);
}

fn identity_list(entries: &[(&[u8], &[u8], &[u8])]) -> Vec<u8> {
  let mut output = vec![1];
  output.extend_from_slice(&(entries.len() as u16).to_be_bytes());
  for (cert, key, issuer) in entries {
    let cert = CertificateDer::from_pem_slice(cert).unwrap();
    let key = PrivateKeyDer::from_pem_slice(key).unwrap();
    for bytes in [cert.as_ref(), key.secret_der()] {
      output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
      output.extend_from_slice(bytes);
    }
    output.extend_from_slice(&1u16.to_be_bytes());
    output.extend_from_slice(&(issuer.len() as u16).to_be_bytes());
    output.extend_from_slice(issuer);
  }
  output
}

#[test]
#[cfg_attr(miri, ignore = "AWS-LC key loading")]
fn identity_lists_reject_truncation_trailing_data_and_conflicting_flags() {
  let owner = Owner::new();
  let valid = identity_list(&[(CLIENT_CERT, CLIENT_KEY, b"issuer")]);
  for length in 0..valid.len() {
    assert_eq!(context(owner.0, ENGINE_CLIENT_IDENTITIES, CERT, &valid[..length]), 0);
  }
  for flags in [
    ENGINE_CLIENT_IDENTITIES | ENGINE_SERVER,
    ENGINE_CLIENT_IDENTITIES | ENGINE_CLIENT_IDENTITY,
  ] {
    assert_eq!(context(owner.0, flags, CERT, &valid), 0);
  }
  let mut trailing = valid;
  trailing.push(0);
  assert_eq!(context(owner.0, ENGINE_CLIENT_IDENTITIES, CERT, &trailing), 0);
  assert_eq!(context(owner.0, ENGINE_CLIENT_IDENTITIES, CERT, &[1, 0, 0]), 0);
  assert_eq!(context(owner.0, ENGINE_CLIENT_IDENTITIES, CERT, &[1, 0, 65]), 0);
  assert_eq!(elide_transport_owner_used(owner.0), 0);
}

fn context(workload: u64, flags: u32, certificates: &[u8], key: &[u8]) -> u64 {
  // SAFETY: Certificate, key and ALPN pointers name live slices with their exact lengths.
  unsafe {
    elide_transport_engine_context_new(
      workload,
      flags,
      certificates.as_ptr(),
      certificates.len() as u64,
      key.as_ptr(),
      key.len() as u64,
      ALPN.as_ptr(),
      ALPN.len() as u64,
    )
  }
}

fn engine(workload: u64, context: u64, name: Option<&str>) -> u64 {
  let name = name.unwrap_or_default().as_bytes();
  // SAFETY: The configuration is retained; the name pointer and its exact byte length remain live for the call.
  unsafe { elide_transport_engine_new(workload, context, name.as_ptr(), name.len() as u64) }
}

struct Owner(u64);

impl Owner {
  fn new() -> Self {
    let owner = elide_transport_owner_new(16 * 1024 * 1024);
    assert_ne!(owner, 0);
    Self(owner)
  }
}

impl Drop for Owner {
  fn drop(&mut self) {
    assert_eq!(elide_transport_owner_release(self.0), 0);
  }
}

struct Packed(i64);

impl Packed {
  fn consumed(&self) -> usize {
    (self.0 & 0xFF_FFFF) as usize
  }
  fn produced(&self) -> usize {
    (self.0 >> 24 & 0xFF_FFFF) as usize
  }
  fn status(&self) -> i64 {
    self.0 >> 48 & 3
  }
  fn handshake(&self) -> i64 {
    self.0 >> 50 & 7
  }
  fn flag(&self, bit: u32) -> bool {
    self.0 & 1 << bit != 0
  }
}

fn wrap(engine: u64, source: &[u8], wire: &mut Vec<u8>) -> Packed {
  let mut out = vec![0u8; 32 * 1024];
  // SAFETY: Input and output are disjoint live slices, or sentinel ranges rejected before dereferencing.
  let result = unsafe {
    elide_transport_engine_wrap(
      engine,
      source.as_ptr(),
      source.len() as u64,
      out.as_mut_ptr(),
      out.len() as u64,
    )
  };
  assert!(result >= 0, "wrap failed: {result}");
  let packed = Packed(result);
  wire.extend_from_slice(&out[..packed.produced()]);
  packed
}

/// Feed whole records until the input is exhausted; returns the last packed result.
fn unwrap(engine: u64, wire: &mut Vec<u8>, plaintext: &mut Vec<u8>, shared: bool) -> Result<Vec<Packed>, i64> {
  let mut results = Vec::new();
  loop {
    let mut out = vec![0u8; 32 * 1024];
    // SAFETY: Input and output are disjoint live slices, or empty ranges which require no storage.
    let result = unsafe {
      elide_transport_engine_unwrap(
        engine,
        wire.as_mut_ptr(),
        wire.len() as u64,
        out.as_mut_ptr(),
        out.len() as u64,
        if shared { ENGINE_SOURCE_SHARED } else { 0 },
      )
    };
    if result < 0 {
      return Err(result);
    }
    let packed = Packed(result);
    wire.drain(..packed.consumed());
    plaintext.extend_from_slice(&out[..packed.produced()]);
    let done = packed.consumed() == 0 && packed.produced() == 0;
    results.push(packed);
    if done {
      return Ok(results);
    }
  }
}

fn info(engine: u64, kind: u32, index: u32) -> (i32, Vec<u8>) {
  let mut out = vec![0u8; 4096];
  // SAFETY: The fixture owns the engine; output is a live writable slice or null with zero capacity.
  let length = unsafe { elide_transport_engine_info(engine, kind, index, out.as_mut_ptr(), out.len() as u64) };
  out.truncate(length.max(0) as usize);
  (length, out)
}

fn handshake(client: u64, server: u64) -> usize {
  let (mut to_server, mut to_client, mut sink) = (Vec::new(), Vec::new(), Vec::new());
  let mut finished = 0;
  for _ in 0..8 {
    let packed = wrap(client, &[], &mut to_server);
    finished += (packed.handshake() == FINISHED) as usize;
    for packed in unwrap(server, &mut to_server, &mut sink, false).unwrap() {
      finished += (packed.handshake() == FINISHED) as usize;
    }
    let packed = wrap(server, &[], &mut to_client);
    finished += (packed.handshake() == FINISHED) as usize;
    for packed in unwrap(client, &mut to_client, &mut sink, true).unwrap() {
      finished += (packed.handshake() == FINISHED) as usize;
    }
    let state = |engine| Packed(elide_transport_engine_control(engine, 0));
    if !state(client).flag(55) && !state(server).flag(55) && finished == 2 {
      return finished;
    }
  }
  panic!("handshake did not finish: {finished}");
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn engine_abi_negotiates_session_details_from_pem_and_der() {
  let owner = Owner::new();
  let der_cert = CertificateDer::from_pem_slice(CERT).unwrap();
  let der_key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
  for der in [false, true] {
    let (flags, cert, key) = if der {
      (ENGINE_DER, der_cert.to_vec(), der_key.secret_der().to_vec())
    } else {
      (0, CERT.to_vec(), KEY.to_vec())
    };
    let client_context = context(owner.0, flags, &cert, &[]);
    let server_context = context(owner.0, flags | ENGINE_SERVER, &cert, &key);
    assert_ne!(client_context, 0);
    assert_ne!(server_context, 0);
    let client = engine(owner.0, client_context, Some("localhost"));
    let server = engine(owner.0, server_context, None);
    assert_ne!(client, 0);
    assert_ne!(server, 0);
    // Engines outlive their contexts.
    assert_eq!(elide_transport_engine_context_release(client_context), 0);
    assert_eq!(elide_transport_engine_context_release(server_context), 0);
    assert_eq!(elide_transport_engine_context_release(server_context), INVALID);
    assert_eq!(Packed(elide_transport_engine_control(client, 0)).handshake(), 0);
    assert_eq!(Packed(elide_transport_engine_control(client, 1)).handshake(), 3);
    assert_eq!(handshake(client, server), 2);

    assert_eq!(info(client, 0, 0).1, b"h2");
    assert_eq!(info(server, 0, 0).1, b"h2");
    assert_eq!(info(client, 1, 0).0, 0x0304);
    assert_ne!(info(server, 2, 0).0, 0);
    assert_eq!(info(client, 3, 0).0, 1);
    assert_eq!(info(client, 4, 0).1, der_cert.as_ref());
    assert_eq!(info(client, 4, 1).0, INVALID);
    assert_eq!(info(server, 3, 0).0, 0);
    let id = info(client, 6, 0).1;
    assert_eq!(id.len(), 32);
    assert_eq!(info(client, 6, 0).1, id);

    // Data crosses threads: engines are caller-serialized, not thread-owned.
    let payload: Vec<u8> = (0..50_000u32).map(|i| i as u8).collect();
    let mut wire = Vec::new();
    let expected = payload.clone();
    let received = std::thread::spawn(move || {
      let mut consumed = 0;
      while consumed < payload.len() {
        consumed += wrap(client, &payload[consumed..], &mut wire).consumed();
      }
      let mut received = Vec::new();
      unwrap(server, &mut wire, &mut received, false).unwrap();
      received
    })
    .join()
    .unwrap();
    assert_eq!(received, expected);

    assert_eq!(Packed(elide_transport_engine_control(client, 2)).handshake(), 3);
    let mut wire = Vec::new();
    let closed = wrap(client, b"late", &mut wire);
    assert_eq!((closed.status(), closed.consumed()), (CLOSED, 0));
    assert!(closed.flag(54));
    let results = unwrap(server, &mut wire, &mut Vec::new(), true).unwrap();
    assert_eq!(results[0].status(), CLOSED);
    assert!(results[0].flag(53));
    assert!(!results[0].flag(54));
    assert!(!Packed(elide_transport_engine_control(server, 3)).flag(56));
    assert!(Packed(elide_transport_engine_control(client, 3)).flag(56));
    assert_eq!(elide_transport_engine_release(client), 0);
    assert_eq!(elide_transport_engine_release(server), 0);
    assert_eq!(elide_transport_engine_release(server), INVALID);
    assert_eq!(elide_transport_engine_control(server, 0), INVALID as i64);
  }
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn engine_abi_rejects_invalid_inputs_and_reports_failures() {
  let owner = Owner::new();
  assert_eq!(context(owner.0, 0, b"not a certificate", &[]), 0);
  assert_eq!(context(owner.0, ENGINE_SERVER, CERT, b"bad key"), 0);
  assert_eq!(context(owner.0, ENGINE_DER, &[0x30, 0x82, 0xff], &[]), 0);
  assert_eq!(context(owner.0, 8, CERT, &[]), 0);
  let client_context = context(owner.0, 0, CERT, &[]);
  let server_context = context(owner.0, ENGINE_SERVER, CERT, KEY);
  assert_eq!(engine(owner.0, client_context, None), 0);
  assert_eq!(engine(owner.0, client_context, Some("bad name!")), 0);
  let client = engine(owner.0, client_context, Some("example.com"));
  let server = engine(owner.0, server_context, None);
  let (mut to_server, mut to_client) = (Vec::new(), Vec::new());
  wrap(client, &[], &mut to_server);
  unwrap(server, &mut to_server, &mut Vec::new(), false).unwrap();
  wrap(server, &[], &mut to_client);
  assert_eq!(
    unwrap(client, &mut to_client, &mut Vec::new(), false).err(),
    Some(ENGINE_FAILED)
  );
  let (_, failure) = info(client, 5, 0);
  assert!(String::from_utf8(failure).unwrap().contains("certificate"));
  let mut alert = Vec::new();
  let packed = wrap(client, &[], &mut alert);
  assert!(!alert.is_empty());
  assert_eq!(packed.status(), CLOSED);
  assert!(packed.flag(53) && packed.flag(54));
  let mut garbage = b"GET / HTTP/1.1\r\n\r\n".to_vec();
  assert_eq!(
    unwrap(server, &mut garbage, &mut Vec::new(), true).err(),
    Some(ENGINE_FAILED)
  );
  assert_eq!(
    // SAFETY: Input and output are disjoint live slices, or sentinel ranges rejected before dereferencing.
    unsafe { elide_transport_engine_wrap(client, std::ptr::null(), 1, std::ptr::null_mut(), 0) },
    INVALID as i64
  );
  assert_eq!(
    // SAFETY: Input and output are disjoint live slices, or sentinel ranges rejected before dereferencing.
    unsafe { elide_transport_engine_wrap(0, std::ptr::null(), 0, std::ptr::null_mut(), 0) },
    INVALID as i64
  );
  assert_eq!(elide_transport_engine_control(client, 9), INVALID as i64);
  for handle in [client, server] {
    assert_eq!(elide_transport_engine_release(handle), 0);
  }
  for handle in [client_context, server_context] {
    assert_eq!(elide_transport_engine_context_release(handle), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "rustls contexts call into AWS-LC")]
fn engine_workloads_reject_cross_owner_use_and_close_independently() {
  let first = Owner::new();
  let sibling = Owner::new();
  assert_eq!(context(0, 0, CERT, &[]), 0);
  let config = context(first.0, 0, CERT, &[]);
  let sibling_config = context(sibling.0, ENGINE_SERVER, CERT, KEY);
  assert_ne!(config, 0);
  assert_ne!(sibling_config, 0);
  assert_eq!(engine(sibling.0, config, Some("localhost")), 0);
  assert_eq!(engine(0, config, Some("localhost")), 0);
  let active = engine(first.0, config, Some("localhost"));
  let other = engine(sibling.0, sibling_config, None);
  assert_ne!(active, 0);
  assert_ne!(other, 0);
  assert_eq!(handshake(active, other), 2);
  let retained = elide_transport_owner_used(first.0);
  assert!(retained > 0);
  assert!(elide_transport_owner_used(sibling.0) > 0);
  assert_eq!(elide_transport_workload_close(first.0), 0);
  assert_eq!(elide_transport_owner_used(first.0), retained);
  assert_eq!(context(first.0, 0, CERT, &[]), 0);
  assert_eq!(engine(first.0, config, Some("localhost")), 0);
  assert_eq!(elide_transport_engine_control(active, 1), ENGINE_FAILED);
  assert_eq!(
    // SAFETY: Input and output are disjoint live slices, or sentinel ranges rejected before dereferencing.
    unsafe { elide_transport_engine_wrap(active, std::ptr::null(), 0, std::ptr::null_mut(), 0) },
    ENGINE_FAILED
  );
  assert_eq!(
    // SAFETY: Input and output are disjoint live slices, or empty ranges which require no storage.
    unsafe { elide_transport_engine_unwrap(active, std::ptr::null_mut(), 0, std::ptr::null_mut(), 0, 0) },
    ENGINE_FAILED
  );
  let mut wire = Vec::new();
  assert!(wrap(other, b"sibling remains open", &mut wire).produced() > 0);
  // Cleanup and diagnostics remain available after admission stops.
  assert!(elide_transport_engine_control(active, 2) >= 0);
  for handle in [active, other] {
    assert_eq!(elide_transport_engine_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_used(first.0), 0);
  assert_eq!(elide_transport_owner_used(sibling.0), 0);
  for handle in [config, sibling_config] {
    assert_eq!(elide_transport_engine_context_release(handle), 0);
  }
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn explicitly_insecure_engine_needs_no_trust_anchors_and_accepts_a_name_mismatch() {
  let owner = Owner::new();
  assert_eq!(context(owner.0, ENGINE_SERVER | ENGINE_INSECURE, CERT, KEY), 0);
  assert_eq!(context(owner.0, 0, &[], &[]), 0);
  let client_context = context(owner.0, ENGINE_INSECURE, &[], &[]);
  let server_context = context(owner.0, ENGINE_SERVER, CERT, KEY);
  assert_ne!(client_context, 0);
  let client = engine(owner.0, client_context, Some("wrong.invalid"));
  let server = engine(owner.0, server_context, None);
  assert_eq!(handshake(client, server), 2);
  let mut wire = Vec::new();
  wrap(client, b"explicit trust override", &mut wire);
  let mut plaintext = Vec::new();
  unwrap(server, &mut wire, &mut plaintext, false).unwrap();
  assert_eq!(plaintext, b"explicit trust override");
  for handle in [client, server] {
    assert_eq!(elide_transport_engine_release(handle), 0);
  }
  for handle in [client_context, server_context] {
    assert_eq!(elide_transport_engine_context_release(handle), 0);
  }
}

fn policy_context(workload: u64, flags: u32, policy: &[u8]) -> u64 {
  let mut alpn = policy.to_vec();
  alpn.extend_from_slice(ALPN);
  // SAFETY: Certificate, key and ALPN pointers name live slices with their exact lengths.
  unsafe {
    elide_transport_engine_context_new(
      workload,
      flags | ENGINE_POLICY,
      CERT.as_ptr(),
      CERT.len() as u64,
      KEY.as_ptr(),
      if flags & ENGINE_SERVER != 0 {
        KEY.len() as u64
      } else {
        0
      },
      alpn.as_ptr(),
      alpn.len() as u64,
    )
  }
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn engine_context_policy_restricts_protocols_and_cipher_suites() {
  let owner = Owner::new();
  for (policy, version, suite) in [
    (&[1, 1, 1, 0xc0, 0x2f][..], 0x0303, 0xc02f),
    (&[1, 2, 1, 0x13, 0x02][..], 0x0304, 0x1302),
    (&[1, 2, 2, 0x13, 0x01, 0x13, 0x02][..], 0x0304, 0x1301),
  ] {
    let client_context = policy_context(owner.0, 0, policy);
    let server_context = policy_context(owner.0, ENGINE_SERVER, policy);
    assert_ne!(client_context, 0);
    assert_ne!(server_context, 0);
    let client = engine(owner.0, client_context, Some("localhost"));
    let server = engine(owner.0, server_context, None);
    assert_eq!(handshake(client, server), 2);
    for handle in [client, server] {
      assert_eq!(info(handle, 1, 0).0, version);
      assert_eq!(info(handle, 2, 0).0, suite);
      assert_eq!(info(handle, 0, 0).1, b"h2");
      assert_eq!(elide_transport_engine_release(handle), 0);
    }
    assert_eq!(elide_transport_engine_context_release(client_context), 0);
    assert_eq!(elide_transport_engine_context_release(server_context), 0);
  }
  for invalid in [
    &[2, 3, 0][..],
    &[1, 0, 0][..],
    &[1, 4, 0][..],
    &[1, 3, 1, 0xff, 0xff][..],
    &[1, 3, 2, 0x13, 0x01, 0x13, 0x01][..],
    &[1, 1, 1, 0x13, 0x01][..],
    &[1, 2, 1, 0xc0, 0x2f][..],
    &[1, 3, 255][..],
  ] {
    assert_eq!(policy_context(owner.0, 0, invalid), 0, "{invalid:?}");
  }
}
