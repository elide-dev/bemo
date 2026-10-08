use std::hint::black_box;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bemo::buffer::Budget;
use bemo::tls::{Action, Session};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};

struct Pair {
  client: Session,
  server: rustls::ServerConnection,
  incoming: Vec<u8>,
  transmitted: bool,
}

impl Pair {
  fn new(client: Arc<rustls::ClientConfig>, server: Arc<rustls::ServerConfig>) -> Self {
    Self {
      client: Session::client(
        client,
        ServerName::try_from("localhost").unwrap(),
        Budget::new(1024 * 1024),
      )
      .unwrap(),
      server: {
        let mut peer = rustls::ServerConnection::new(server).unwrap();
        peer.set_buffer_limit(None);
        peer
      },
      incoming: Vec::new(),
      transmitted: false,
    }
  }

  fn roundtrip(&mut self, payload: &[u8]) {
    let mut sent = 0;
    let mut received = Vec::new();
    let mut echoed = Vec::new();
    for _ in 0..1000 {
      let action = if self.transmitted {
        self.transmitted = false;
        Action::Transmitted
      } else if sent < payload.len() {
        Action::Write(&payload[sent..])
      } else {
        Action::Continue
      };
      let step = self.client.step(&mut self.incoming, action).unwrap();
      self.incoming.drain(..step.discard);
      sent += step.accepted;
      if let Some(output) = step.output {
        let mut wire = Cursor::new(output.as_ref());
        while wire.position() < output.as_ref().len() as u64 {
          assert_ne!(self.server.read_tls(&mut wire).unwrap(), 0);
          self.server.process_new_packets().unwrap();
          let mut fragment = [0; 16384];
          while let Ok(count) = self.server.reader().read(&mut fragment) {
            if count == 0 {
              break;
            }
            received.extend_from_slice(&fragment[..count]);
            self.server.writer().write_all(&fragment[..count]).unwrap();
          }
        }
        self.transmitted = true;
      }
      if let Some(plaintext) = step.plaintext {
        echoed.extend_from_slice(plaintext.as_ref());
      }
      let mut fragment = [0; 16384];
      while let Ok(count) = self.server.reader().read(&mut fragment) {
        if count == 0 {
          break;
        }
        received.extend_from_slice(&fragment[..count]);
        self.server.writer().write_all(&fragment[..count]).unwrap();
      }
      self.server.write_tls(&mut self.incoming).unwrap();
      if echoed.len() == payload.len() {
        assert_eq!(received, payload);
        assert_eq!(echoed, payload);
        return;
      }
    }
    panic!("TLS roundtrip did not complete");
  }
}

// Bound each established session's record count, including Criterion warmup.
// TLS 1.2 cannot refresh traffic keys; handshakes stay outside the returned time.
fn measure_established(
  iterations: u64,
  client: &Arc<rustls::ClientConfig>,
  server: &Arc<rustls::ServerConfig>,
  mut operation: impl FnMut(&mut Pair),
) -> Duration {
  let mut remaining = iterations;
  let mut elapsed = Duration::ZERO;
  while remaining > 0 {
    let count = remaining.min(4096);
    let mut pair = Pair::new(client.clone(), server.clone());
    pair.roundtrip(b"warmup");
    let start = Instant::now();
    for _ in 0..count {
      operation(&mut pair);
    }
    elapsed += start.elapsed();
    remaining -= count;
  }
  elapsed
}

fn tls(c: &mut Criterion) {
  for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
    let cert = CertificateDer::from_pem_slice(include_bytes!("../tests/fixtures/localhost-cert.pem")).unwrap();
    let key = PrivateKeyDer::from_pem_slice(include_bytes!("../tests/fixtures/localhost-key.pem")).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.clone()).unwrap();
    let mut client = rustls::ClientConfig::builder_with_provider(provider.clone())
      .with_protocol_versions(&[version])
      .unwrap()
      .with_root_certificates(roots)
      .with_no_client_auth();
    client.resumption = rustls::client::Resumption::disabled();
    let server = Arc::new(
      rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[version])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap(),
    );
    let client = Arc::new(client);
    let mut group = c.benchmark_group(format!("tls/{:?}", version.version));
    group.throughput(Throughput::Elements(1));
    group.bench_function("full-handshake-and-first-record", |b| {
      b.iter(|| Pair::new(client.clone(), server.clone()).roundtrip(black_box(b"hello")));
    });
    for size in [64, 4096, 65536, 131072] {
      let payload = vec![42; size];
      group.throughput(Throughput::Bytes(size as u64));
      group.bench_function(BenchmarkId::new("established-encrypt", size), |b| {
        b.iter_custom(|iterations| {
          measure_established(iterations, &client, &server, |established| {
            let mut accepted = 0;
            while accepted < payload.len() {
              let step = established
                .client
                .step(&mut [], Action::Write(black_box(&payload[accepted..])))
                .unwrap();
              accepted += step.accepted;
              if let Some(output) = step.output {
                black_box(output);
              }
              if step.state == bemo::tls::State::NeedTransmit {
                black_box(established.client.step(&mut [], Action::Transmitted).unwrap());
              }
            }
          })
        });
      });
      group.bench_function(BenchmarkId::new("record-roundtrip", size), |b| {
        b.iter_custom(|iterations| {
          measure_established(iterations, &client, &server, |pair| pair.roundtrip(black_box(&payload)))
        });
      });
    }
    group.finish();
  }
}
criterion_group!(benches, tls);
criterion_main!(benches);
