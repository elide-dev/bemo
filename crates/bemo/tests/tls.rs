use bemo::buffer::Budget;
use bemo::tls::{Action, Session, State};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

fn configs() -> (Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>) {
  configs_with_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
}

fn configs_with_versions(
  versions: &[&'static rustls::SupportedProtocolVersion],
) -> (Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>) {
  let cert = CertificateDer::from_pem_slice(include_bytes!("fixtures/localhost-cert.pem")).unwrap();
  let key = PrivateKeyDer::from_pem_slice(include_bytes!("fixtures/localhost-key.pem")).unwrap();
  let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
  let mut roots = rustls::RootCertStore::empty();
  roots.add(cert.clone()).unwrap();
  let mut client = rustls::ClientConfig::builder_with_provider(provider.clone())
    .with_protocol_versions(versions)
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
  let mut server = rustls::ServerConfig::builder_with_provider(provider)
    .with_protocol_versions(versions)
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
  client.alpn_protocols = vec![b"h2".to_vec()];
  server.alpn_protocols = vec![b"h2".to_vec()];
  (Arc::new(client), Arc::new(server))
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn unbuffered_tls_interoperates_with_buffered_peer_and_retains_plaintext() {
  let (client, server) = configs();
  let budget = Budget::new(1024 * 1024);
  let mut client = Session::client(client, ServerName::try_from("localhost").unwrap(), budget.clone()).unwrap();
  let mut server = rustls::ServerConnection::new(server).unwrap();
  let mut incoming = Vec::new();
  let mut transmitted = false;
  let mut sent = false;
  let mut reply = None;
  let mut request = Vec::new();
  let mut replied = false;
  for _ in 0..100 {
    let action = if transmitted {
      transmitted = false;
      Action::Transmitted
    } else if !sent {
      Action::Write(b"hello")
    } else {
      Action::Continue
    };
    let step = client.step(&mut incoming, action).unwrap();
    incoming.drain(..step.discard);
    sent |= step.accepted != 0;
    if let Some(output) = step.output {
      server.read_tls(&mut Cursor::new(output.as_ref())).unwrap();
      server.process_new_packets().unwrap();
      transmitted = true;
    }
    if let Some(plaintext) = step.plaintext {
      reply = Some(plaintext);
      break;
    }
    let mut fragment = [0; 1];
    while let Ok(1) = server.reader().read(&mut fragment) {
      request.extend_from_slice(&fragment);
    }
    if request.len() == 5 && !replied {
      assert_eq!(&request, b"hello");
      server.writer().write_all(b"world").unwrap();
      replied = true;
    }
    server.write_tls(&mut incoming).unwrap();
  }
  assert!(sent);
  assert_eq!(client.alpn_protocol(), Some(b"h2".as_slice()));
  let reply = reply.expect("TLS reply");
  drop(client);
  assert_eq!(reply.as_ref(), b"world");
  assert!(budget.used() > 0);
  drop(reply);
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn client_hello_is_encoded_without_a_socket_or_executor() {
  let (client, _) = configs();
  let mut session = Session::client(client, ServerName::try_from("localhost").unwrap(), Budget::new(65536)).unwrap();
  let step = session.step(&mut [], Action::Continue).unwrap();
  assert_eq!(step.state, State::Encoded);
  assert!(step.output.unwrap().as_ref().len() > 5);
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn tls12_small_records_fit_their_actual_budget() {
  small_records(&rustls::version::TLS12);
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn tls13_small_records_fit_their_actual_budget() {
  small_records(&rustls::version::TLS13);
}

fn small_records(version: &'static rustls::SupportedProtocolVersion) {
  let (client, server) = configs_with_versions(&[version]);
  let budget = Budget::new(4096);
  let mut client = Session::client(client, ServerName::try_from("localhost").unwrap(), budget.clone()).unwrap();
  let mut server = rustls::ServerConnection::new(server).unwrap();
  let mut incoming = Vec::new();
  let mut transmitted = false;
  for length in [1, 63, 2048] {
    let payload: Vec<u8> = (0..length).map(|offset| offset as u8).collect();
    let mut received = Vec::new();
    let mut sent = false;
    for _ in 0..100 {
      let action = if transmitted {
        transmitted = false;
        Action::Transmitted
      } else if !sent {
        Action::Write(&payload)
      } else {
        Action::Continue
      };
      let step = client.step(&mut incoming, action).unwrap();
      incoming.drain(..step.discard);
      if step.accepted != 0 {
        assert_eq!(step.accepted, payload.len());
        assert!(!sent);
        sent = true;
      }
      if let Some(output) = step.output {
        assert_eq!(budget.used(), output.as_ref().len(), "output allocation is oversized");
        server.read_tls(&mut Cursor::new(output.as_ref())).unwrap();
        server.process_new_packets().unwrap();
        transmitted = true;
      }
      let mut fragment = [0; 256];
      while let Ok(count) = server.reader().read(&mut fragment) {
        if count == 0 {
          break;
        }
        received.extend_from_slice(&fragment[..count]);
      }
      server.write_tls(&mut incoming).unwrap();
      if received.len() == payload.len() {
        break;
      }
    }
    assert!(sent);
    assert_eq!(received, payload);
    assert_eq!(budget.used(), 0);
    if length == 1 && version.version == rustls::ProtocolVersion::TLSv1_3 {
      server.refresh_traffic_keys().unwrap();
      server.write_tls(&mut incoming).unwrap();
    }
  }
  assert_eq!(server.protocol_version(), Some(version.version));
  let mut closed = false;
  for _ in 0..100 {
    let action = if transmitted {
      transmitted = false;
      Action::Transmitted
    } else {
      Action::Close
    };
    let step = client.step(&mut incoming, action).unwrap();
    incoming.drain(..step.discard);
    if let Some(output) = step.output {
      assert_eq!(budget.used(), output.as_ref().len());
      server.read_tls(&mut Cursor::new(output.as_ref())).unwrap();
      closed = server.process_new_packets().unwrap().peer_has_closed();
      if closed {
        break;
      }
      transmitted = true;
    }
    server.write_tls(&mut incoming).unwrap();
  }
  assert!(closed, "close_notify was not delivered");
  assert_eq!(budget.used(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "rustls configuration builds an AWS-LC provider")]
fn early_data_configurations_are_rejected() {
  let (client, server) = configs();
  let mut client = (*client).clone();
  client.enable_early_data = true;
  let error = Session::client(
    Arc::new(client),
    ServerName::try_from("localhost").unwrap(),
    Budget::new(4096),
  )
  .err()
  .expect("early data is unsupported");
  assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
  let mut server = (*server).clone();
  server.max_early_data_size = 1024;
  let error = Session::server(Arc::new(server), Budget::new(4096))
    .err()
    .expect("early data is unsupported");
  assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn a_failed_server_session_stays_failed() {
  let (_, server) = configs();
  let mut session = Session::server(server, Budget::new(65536)).unwrap();
  assert!(session.is_handshaking());
  assert_eq!(session.alpn_protocol(), None);
  let mut garbage = b"GET / HTTP/1.1\r\n\r\n".to_vec();
  let error = session
    .step(&mut garbage, Action::Continue)
    .err()
    .expect("plaintext HTTP is not a TLS record");
  assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
  let error = session
    .step(&mut [], Action::Continue)
    .err()
    .expect("a failed session is terminal");
  assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn a_fresh_server_session_waits_for_the_client_hello() {
  let (_, server) = configs();
  let mut session = Session::server(server, Budget::new(65536)).unwrap();
  let step = session.step(&mut [], Action::Continue).unwrap();
  assert_eq!(step.state, State::NeedRead);
  assert!(step.output.is_none() && step.plaintext.is_none());
  assert_eq!((step.discard, step.accepted), (0, 0));
  // Writes before the handshake allows application data are not accepted.
  let step = session.step(&mut [], Action::Write(b"early")).unwrap();
  assert_eq!(step.accepted, 0);
}

#[test]
#[cfg_attr(miri, ignore = "rustls handshakes call into AWS-LC")]
fn batched_records_are_bounded_ordered_and_keep_their_storage_until_release() {
  for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
    let (config, peer) = configs_with_versions(&[version]);
    for limit in [1024 * 1024, 32768] {
      let budget = Budget::new(limit);
      let mut session = Session::client(config.clone(), ServerName::try_from("localhost").unwrap(), budget).unwrap();
      let mut peer = rustls::ServerConnection::new(peer.clone()).unwrap();
      peer.set_buffer_limit(None);
      let mut incoming = Vec::new();
      let mut transmitted = false;
      let payload: Vec<u8> = (0..bemo::tls::MAX_WRITE + 1).map(|i| (i % 251) as u8).collect();
      let mut outputs = Vec::new();
      let mut decoded = Vec::new();
      let mut accepted = 0;
      for _ in 0..100 {
        let action = if transmitted {
          transmitted = false;
          Action::Transmitted
        } else {
          Action::Write(&payload[accepted..])
        };
        let step = session.step(&mut incoming, action).unwrap();
        incoming.drain(..step.discard);
        if step.accepted != 0 {
          assert_eq!(
            step.accepted,
            if limit == 32768 {
              16384
            } else {
              (payload.len() - accepted).min(bemo::tls::MAX_WRITE)
            }
          );
          accepted += step.accepted;
          outputs.push(step.output.as_ref().unwrap().clone());
        }
        if let Some(output) = step.output {
          let mut cursor = Cursor::new(output.as_ref());
          while cursor.position() < output.as_ref().len() as u64 {
            peer.read_tls(&mut cursor).unwrap();
            peer.process_new_packets().unwrap();
            let mut fragment = [0; 16384];
            while let Ok(length) = peer.reader().read(&mut fragment) {
              if length == 0 {
                break;
              }
              decoded.extend_from_slice(&fragment[..length]);
            }
          }
          transmitted = true;
        }
        peer.write_tls(&mut incoming).unwrap();
        if accepted == payload.len() || (limit == 32768 && accepted != 0) {
          break;
        }
      }
      if limit == 32768 {
        assert_eq!(accepted, 16384);
        assert_eq!(decoded, payload[..16384]);
        continue;
      }
      assert_eq!(accepted, payload.len());
      assert_eq!(outputs.len(), 2, "large response uses two bounded output allocations");
      let first = outputs[0].as_ref().to_vec();
      let mut offset = 0;
      let mut records = 0;
      while offset < first.len() {
        assert_eq!(first[offset], 23);
        let length = u16::from_be_bytes([first[offset + 3], first[offset + 4]]) as usize;
        assert!(length <= 16384 + 2048);
        offset += 5 + length;
        records += 1;
      }
      assert_eq!(offset, first.len());
      assert_eq!(records, 8);
      assert_eq!(decoded, payload);
      drop(session);
      assert_eq!(
        outputs[0].as_ref(),
        first,
        "ciphertext lease survives later writes and session release"
      );
    }
  }
}
