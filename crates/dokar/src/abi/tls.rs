/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Portable TLS configuration and owner-thread sessions, independent of Netty and socket drivers.

use super::*;
use crate::tls::{Action, Session, State};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use std::sync::Arc;

#[derive(Clone)]
enum Config {
  Client(Arc<rustls::ClientConfig>),
  Server(Arc<rustls::ServerConfig>),
}
#[derive(Clone)]
struct OwnedConfig {
  config: Config,
  workload: Workload,
}
static CONFIGS: LazyLock<Mutex<IntMap<u64, OwnedConfig>>> = LazyLock::new(Mutex::default);
struct Tls {
  session: Session,
  input: Option<Buffer>,
  length: usize,
  budget: Budget,
  workload: Workload,
}
thread_local! { static SESSIONS: RefCell<IntMap<u64, Tls>> = RefCell::new(IntMap::default()); }
const MAX_INPUT: usize = 1024 * 1024;

fn frozen(handle: u64) -> Option<FrozenBuffer> {
  match lock(registry(handle)).get(&handle)? {
    Storage::Frozen(buffer) => Some(buffer.clone()),
    _ => None,
  }
}
fn protocols(handle: u64) -> Option<Vec<Vec<u8>>> {
  if handle == 0 {
    return Some(Vec::new());
  }
  let buffer = frozen(handle)?;
  let mut bytes = buffer.as_ref();
  let mut output = Vec::new();
  while !bytes.is_empty() {
    let length = bytes[0] as usize;
    if length == 0 || bytes.len() <= length {
      return None;
    }
    output.push(bytes[1..=length].to_vec());
    bytes = &bytes[length + 1..];
  }
  Some(output)
}

/// Build a verified AWS-LC client context from PEM trust anchors and wire-format ALPN identifiers.
pub fn elide_transport_tls_client(workload: u64, roots: u64, alpn: u64) -> u64 {
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  let Some(roots) = frozen(roots) else { return 0 };
  let Some(alpn) = protocols(alpn) else {
    return 0;
  };
  let mut store = rustls::RootCertStore::empty();
  for certificate in CertificateDer::pem_slice_iter(roots.as_ref()) {
    let Ok(certificate) = certificate else {
      return 0;
    };
    if store.add(certificate).is_err() {
      return 0;
    }
  }
  if store.is_empty() {
    return 0;
  }
  let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
  let Ok(builder) = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions() else {
    return 0;
  };
  let mut config = builder.with_root_certificates(store).with_no_client_auth();
  config.alpn_protocols = alpn;
  let id = identity();
  if id != 0 {
    lock(&CONFIGS).insert(
      id,
      OwnedConfig {
        config: Config::Client(Arc::new(config)),
        workload,
      },
    );
  }
  id
}

/// Build an AWS-LC server context from a PEM certificate chain, PEM private key, and ALPN identifiers.
pub fn elide_transport_tls_server(workload: u64, chain: u64, key: u64, alpn: u64) -> u64 {
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  let (Some(chain), Some(key), Some(alpn)) = (frozen(chain), frozen(key), protocols(alpn)) else {
    return 0;
  };
  let Ok(chain) = CertificateDer::pem_slice_iter(chain.as_ref()).collect::<Result<Vec<_>, _>>() else {
    return 0;
  };
  let Ok(key) = PrivateKeyDer::from_pem_slice(key.as_ref()) else {
    return 0;
  };
  let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
  let Ok(builder) = rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions() else {
    return 0;
  };
  let Ok(mut config) = builder.with_no_client_auth().with_single_cert(chain, key) else {
    return 0;
  };
  config.alpn_protocols = alpn;
  let id = identity();
  if id != 0 {
    lock(&CONFIGS).insert(
      id,
      OwnedConfig {
        config: Config::Server(Arc::new(config)),
        workload,
      },
    );
  }
  id
}

pub(super) fn server_config(workload: u64, context: u64) -> Option<Arc<rustls::ServerConfig>> {
  let configs = lock(&CONFIGS);
  let context = configs.get(&context)?;
  if !context.workload.admits(workload) {
    return None;
  }
  match &context.config {
    Config::Server(config) => Some(config.clone()),
    Config::Client(_) => None,
  }
}

/// Release a context; existing sessions retain their immutable configuration.
pub fn elide_transport_tls_context_release(context: u64) -> i32 {
  if lock(&CONFIGS).remove(&context).is_some() {
    0
  } else {
    INVALID
  }
}

/// Create an owner-thread session of `workload` charging `owner`. Clients require a frozen UTF-8
/// verification name; servers use zero. Once the workload closes, the session only shuts down.
pub fn elide_transport_tls_new(workload: u64, context: u64, owner: u64, name: u64) -> u64 {
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  let Some(config) = lock(&CONFIGS).get(&context).cloned() else {
    return 0;
  };
  if !config.workload.admits(workload.id) {
    return 0;
  }
  let Some(budget) = budget(owner) else {
    return 0;
  };
  let session = match config.config {
    Config::Client(config) => {
      let Some(name) = frozen(name) else { return 0 };
      let Ok(name) = std::str::from_utf8(name.as_ref()) else {
        return 0;
      };
      let Ok(name) = ServerName::try_from(name.to_owned()) else {
        return 0;
      };
      Session::client(config, name, budget.clone())
    }
    Config::Server(config) => Session::server(config, budget.clone()),
  };
  let Ok(session) = session else { return 0 };
  let id = identity();
  if id != 0 {
    SESSIONS.with(|sessions| {
      sessions.borrow_mut().insert(
        id,
        Tls {
          session,
          input: None,
          length: 0,
          budget,
          workload,
        },
      )
    });
  }
  id
}

/// Transfer a completed mutable receive into TLS; failure leaves the handle owned by the caller.
pub fn elide_transport_tls_feed(session: u64, input: u64, length: u64) -> i32 {
  let Ok(length) = usize::try_from(length) else {
    return INVALID;
  };
  SESSIONS.with(|sessions| {
    let mut sessions = sessions.borrow_mut();
    let Some(tls) = sessions.get_mut(&session) else {
      return INVALID;
    };
    if tls.workload.closed() || length > MAX_INPUT - tls.length {
      return INVALID;
    }
    let mut buffers = lock(registry(input));
    let Some(Storage::Mutable(buffer)) = buffers.get(&input) else {
      return INVALID;
    };
    if length > IoBuf::buf_len(buffer) {
      return INVALID;
    }
    if tls.length == 0 {
      let Some(Storage::Mutable(buffer)) = buffers.remove(&input) else {
        return INVALID;
      };
      tls.input = Some(buffer.into_private());
    } else {
      let total = tls.length + length;
      if tls.input.as_mut().is_none_or(|b| b.buf_capacity() < total) {
        let Ok(mut grown) = Buffer::new(total, tls.budget.clone()) else {
          return INVALID;
        };
        if let Some(previous) = &tls.input
          && grown.write(0, previous.as_init()).is_err()
        {
          return INVALID;
        }
        tls.input = Some(grown);
      }
      let target = tls.input.as_mut().unwrap();
      if target.write(tls.length, &buffer.as_init()[..length]).is_err() {
        return INVALID;
      }
      buffers.remove(&input);
    }
    tls.length += length;
    0
  })
}

/// Advance TLS. Actions: 0 progress, 1 acknowledge handshake transmission, 2 write, 3 close-notify.
/// A closed workload rejects writes.
/// Output is a mutable 32-byte descriptor: state, accepted bytes, encoded handle, plaintext handle (u64).
/// States: 0 need-read, 1 encoded, 2 need-transmit, 3 ready, 4 plaintext, 5 peer-closed, 6 closed.
/// Bit 32 of the state field indicates completed peer authentication, independently of record availability.
pub fn elide_transport_tls_step(
  session: u64,
  action: u32,
  plaintext: u64,
  offset: u64,
  length: u64,
  output: u64,
) -> i32 {
  if action > 3 {
    return INVALID;
  }
  let data = if action == 2 {
    let (Ok(offset), Ok(length)) = (usize::try_from(offset), usize::try_from(length)) else {
      return INVALID;
    };
    let Some(end) = offset.checked_add(length) else {
      return INVALID;
    };
    let Some(buffer) = frozen(plaintext) else {
      return INVALID;
    };
    let Ok(buffer) = FrozenBuffer::slice(&buffer, offset..end) else {
      return INVALID;
    };
    Some(buffer)
  } else {
    None
  };
  let mut buffers = lock(registry(output));
  let Some(Storage::Mutable(buffer)) = buffers.get_mut(&output) else {
    return INVALID;
  };
  if buffer.buf_capacity() < 32 {
    return INVALID;
  }
  let Some(Storage::Mutable(mut descriptor)) = buffers.remove(&output) else {
    return INVALID;
  };
  drop(buffers);
  let result = SESSIONS.with(|sessions| {
    let mut sessions = sessions.borrow_mut();
    let Some(tls) = sessions.get_mut(&session) else {
      return INVALID;
    };
    if data.is_some() && tls.workload.closed() {
      return INVALID;
    }
    let action = match action {
      1 => Action::Transmitted,
      2 => Action::Write(data.as_ref().unwrap().as_ref()),
      3 => Action::Close,
      _ => Action::Continue,
    };
    let input = match &mut tls.input {
      Some(buffer) => &mut buffer.as_mut_slice()[..tls.length],
      None => &mut [],
    };
    let Ok(step) = tls.session.step(input, action) else {
      return -3;
    };
    if step.discard > tls.length {
      return -3;
    }
    input.copy_within(step.discard.., 0);
    tls.length -= step.discard;
    if let Some(buffer) = &mut tls.input {
      unsafe { buffer.set_len(tls.length) };
    }
    let state = match step.state {
      State::NeedRead => 0,
      State::Encoded => 1,
      State::NeedTransmit => 2,
      State::Ready => 3,
      State::Plaintext => 4,
      State::PeerClosed => 5,
      State::Closed => 6,
    };
    let mut fields = [
      state | if tls.session.is_handshaking() { 0 } else { 1 << 32 },
      step.accepted as u64,
      0,
      0,
    ];
    let mut buffers = lock(local());
    for (index, buffer) in [(2, step.output), (3, step.plaintext)] {
      if let Some(buffer) = buffer {
        let id = identity();
        if id == 0 {
          for id in fields[2..].iter().filter(|id| **id != 0) {
            buffers.remove(id);
          }
          return -3;
        }
        let storage = if index == 3 {
          let Ok(mut buffer) = buffer.try_into_mut() else {
            return -3;
          };
          buffer.ensure_init();
          Storage::Mutable(buffer)
        } else {
          Storage::Frozen(buffer)
        };
        buffers.insert(id, storage);
        fields[index] = id;
      }
    }
    for (index, field) in fields.iter().enumerate() {
      descriptor.write(index * 8, &field.to_ne_bytes()).unwrap();
    }
    0
  });
  lock(registry(output)).insert(output, Storage::Mutable(descriptor));
  result
}

/// Copy negotiated ALPN into a mutable output buffer; returns its length, or a negative error.
pub fn elide_transport_tls_protocol(session: u64, output: u64) -> i32 {
  SESSIONS.with(|sessions| {
    let sessions = sessions.borrow();
    let Some(tls) = sessions.get(&session) else {
      return INVALID;
    };
    let protocol = tls.session.alpn_protocol().unwrap_or_default();
    let mut buffers = lock(registry(output));
    let Some(Storage::Mutable(buffer)) = buffers.get_mut(&output) else {
      return INVALID;
    };
    if buffer.write(0, protocol).is_err() {
      return INVALID;
    }
    protocol.len() as i32
  })
}

/// Release a TLS session on its owner thread. Exported plaintext keeps its own lifetime.
pub fn elide_transport_tls_release(session: u64) -> i32 {
  SESSIONS.with(|sessions| {
    if sessions.borrow_mut().remove(&session).is_some() {
      0
    } else {
      INVALID
    }
  })
}
