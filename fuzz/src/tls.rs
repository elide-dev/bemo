use bemo::buffer::Budget;
use bemo::tls::{Action, Session, State};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use std::sync::{Arc, OnceLock};

const CLIENT_DATA: &[u8] = b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n";
const SERVER_DATA: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n";
const ROUNDS: usize = 256;

type Configs = (Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>);

fn configs(index: usize) -> &'static Configs {
  static CONFIGS: OnceLock<[Configs; 3]> = OnceLock::new();
  &CONFIGS.get_or_init(|| {
    let build = |versions: &[&'static rustls::SupportedProtocolVersion]| {
      let cert = CertificateDer::from_pem_slice(include_bytes!("../../crates/bemo/tests/fixtures/localhost-cert.pem"))
        .expect("fixture certificate");
      let key = PrivateKeyDer::from_pem_slice(include_bytes!("../../crates/bemo/tests/fixtures/localhost-key.pem"))
        .expect("fixture key");
      let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
      let mut roots = rustls::RootCertStore::empty();
      roots.add(cert.clone()).expect("fixture root");
      let mut client = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .expect("client versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
      let mut server = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(versions)
        .expect("server versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server identity");
      client.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
      server.alpn_protocols = vec![b"http/1.1".to_vec()];
      (Arc::new(client), Arc::new(server))
    };
    [
      build(&[&rustls::version::TLS13]),
      build(&[&rustls::version::TLS12]),
      build(&[&rustls::version::TLS13, &rustls::version::TLS12]),
    ]
  })[index % 3]
}

/// One direction of the in-memory wire, applying the fuzzer's script to each record.
struct Lane<'a> {
  script: &'a [u8],
  cursor: usize,
  tampered: bool,
}

impl Lane<'_> {
  fn next(&mut self) -> u8 {
    let byte = self.script.get(self.cursor).copied().unwrap_or(0);
    self.cursor += 1;
    byte
  }

  /// Deliver one encoded flight: possibly corrupted, truncated, duplicated, or dropped,
  /// and always fragmented into fuzzer-chosen pieces (which TLS must tolerate).
  fn deliver(&mut self, output: &[u8], incoming: &mut Vec<Vec<u8>>) {
    let mut bytes = output.to_vec();
    let operation = self.next();
    match operation % 16 {
      0 if !bytes.is_empty() => {
        let at = usize::from(self.next()) * bytes.len() / 256;
        bytes[at] ^= 1 << (self.next() % 8);
        self.tampered = true;
      }
      1 if !bytes.is_empty() => {
        bytes.truncate(usize::from(self.next()) * bytes.len() / 256);
        self.tampered = true;
      }
      2 => {
        bytes.extend_from_slice(output);
        self.tampered = true;
      }
      3 => {
        bytes.clear();
        self.tampered = true;
      }
      _ => {}
    }
    let piece = usize::from(operation >> 4) * 64 + 1;
    incoming.extend(bytes.chunks(piece).map(<[u8]>::to_vec));
  }
}

struct Peer {
  session: Session,
  incoming: Vec<u8>,
  pending: Vec<Vec<u8>>,
  transmitted: bool,
  sent: usize,
  received: Vec<u8>,
  failed: bool,
  closed: bool,
}

impl Peer {
  fn new(session: Session) -> Self {
    Self {
      session,
      incoming: Vec::new(),
      pending: Vec::new(),
      transmitted: false,
      sent: 0,
      received: Vec::new(),
      failed: false,
      closed: false,
    }
  }

  /// Deliver each pending fragment with a transition after it; return wire output in order.
  fn step(&mut self, payload: &[u8], close: bool) -> Vec<Vec<u8>> {
    let mut outputs = Vec::new();
    let pieces = std::mem::take(&mut self.pending);
    if pieces.is_empty() {
      self.advance(payload, close, &mut outputs);
    }
    for piece in pieces {
      self.incoming.extend_from_slice(&piece);
      self.advance(payload, close, &mut outputs);
    }
    outputs
  }

  fn advance(&mut self, payload: &[u8], close: bool, outputs: &mut Vec<Vec<u8>>) {
    if self.failed || self.closed {
      return;
    }
    // Like the native HTTP lane, keep acknowledging until a step reports NeedTransmit:
    // a step that surfaces plaintext first does not consume the acknowledgement.
    let acknowledging = self.transmitted;
    let action = if acknowledging {
      Action::Transmitted
    } else if self.sent < payload.len() {
      Action::Write(&payload[self.sent..])
    } else if close {
      Action::Close
    } else {
      Action::Continue
    };
    match self.session.step(&mut self.incoming, action) {
      Ok(step) => {
        assert!(
          step.discard <= self.incoming.len(),
          "discarded more input than supplied"
        );
        self.incoming.drain(..step.discard);
        assert!(step.accepted <= payload.len() - self.sent);
        self.sent += step.accepted;
        if let Some(plaintext) = step.plaintext {
          self.received.extend_from_slice(plaintext.as_ref());
        }
        if acknowledging && step.state == State::NeedTransmit {
          self.transmitted = false;
        }
        if step.state == State::Closed {
          self.closed = true;
        }
        if let Some(output) = step.output {
          self.transmitted = true;
          outputs.push(output.as_ref().to_vec());
        }
      }
      Err(_) => {
        self.failed = true;
        // Errors are terminal: no later transition may succeed.
        assert!(
          self.session.step(&mut self.incoming, Action::Continue).is_err(),
          "session recovered after an error"
        );
      }
    }
  }
}

/// A Bemo client and server exchange application data over a wire the fuzzer controls.
///
/// Authenticated plaintext must be a prefix of what the peer sent, no matter how the wire
/// is fragmented or tampered with. When no byte is altered, both sides must complete and
/// deliver everything. Every native allocation is returned to its budget.
pub fn tls(data: &[u8]) {
  let [versions, flags, script @ ..] = data else {
    return;
  };
  let (client_config, server_config) = configs(usize::from(*versions));
  let (to_server, to_client) = script.split_at(script.len() / 2);
  let budget = Budget::new(4 << 20);
  let name = ServerName::try_from("localhost").expect("fixture server name");
  let mut client = Peer::new(Session::client(client_config.clone(), name, budget.clone()).expect("client session"));
  let mut server = Peer::new(Session::server(server_config.clone(), budget.clone()).expect("server session"));
  let mut upstream = Lane {
    script: to_server,
    cursor: 0,
    tampered: false,
  };
  let mut downstream = Lane {
    script: to_client,
    cursor: 0,
    tampered: false,
  };
  let close = flags & 1 != 0;
  for _ in 0..ROUNDS {
    for output in client.step(CLIENT_DATA, close) {
      upstream.deliver(&output, &mut server.pending);
    }
    for output in server.step(SERVER_DATA, close) {
      downstream.deliver(&output, &mut client.pending);
    }
    let done = |peer: &Peer| peer.failed || peer.closed;
    let idle = client.pending.is_empty() && server.pending.is_empty();
    if (done(&client) && done(&server))
      || (idle && client.received == SERVER_DATA && server.received == CLIENT_DATA && !close)
    {
      break;
    }
  }
  assert!(
    SERVER_DATA.starts_with(&client.received),
    "client accepted unauthenticated plaintext"
  );
  assert!(
    CLIENT_DATA.starts_with(&server.received),
    "server accepted unauthenticated plaintext"
  );
  if !upstream.tampered && !downstream.tampered {
    assert!(!client.failed && !server.failed, "an untampered exchange failed");
    assert_eq!(server.received, CLIENT_DATA, "fragmentation lost client data");
    assert_eq!(client.received, SERVER_DATA, "fragmentation lost server data");
  }
  drop((client, server));
  assert_eq!(budget.used(), 0, "TLS storage leaked from the budget");
}
