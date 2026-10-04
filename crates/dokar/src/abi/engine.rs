/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! `SSLEngine` ABI: immutable contexts and caller-serialized engines, usable from any thread. Wrap
//! and unwrap read and write caller memory directly; no TLS record passes through a handle.

use super::{INVALID, Workload, identity, lock};
use crate::tls::engine::{Engine, Input, Outcome, Status};
use nohash_hasher::IntMap;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use std::sync::{Arc, LazyLock, Mutex};

mod client_identity;

/// Context flag: server identity instead of client trust anchors.
pub const ENGINE_SERVER: u32 = 1;
/// Context flag: certificates are concatenated DER and the key is DER, instead of PEM.
pub const ENGINE_DER: u32 = 2;
/// Client context flag: explicitly disable certificate-chain and hostname verification.
pub const ENGINE_INSECURE: u32 = 4;
/// Context flag: ALPN starts with a versioned protocol/cipher policy prefix.
pub const ENGINE_POLICY: u32 = 8;
/// Client context flag: `key` contains a PEM client certificate chain and private key.
pub const ENGINE_CLIENT_IDENTITY: u32 = 16;
/// Client context flag: `key` contains a versioned identity list with issuer-name hints.
pub const ENGINE_CLIENT_IDENTITIES: u32 = 32;
/// Unwrap flag: the source must not be modified.
pub const ENGINE_SOURCE_SHARED: u32 = 1;
/// Terminal TLS failure; `engine_info` kind `ENGINE_INFO_FAILURE` describes it.
pub const ENGINE_FAILED: i64 = -3;

const LENGTH_MASK: u64 = (1 << 24) - 1;
const INBOUND_DONE: i64 = 1 << 53;
const OUTBOUND_DONE: i64 = 1 << 54;
const HANDSHAKING: i64 = 1 << 55;
const TRUNCATED: i64 = 1 << 56;

const ENGINE_INFO_ALPN: u32 = 0;
const ENGINE_INFO_PROTOCOL: u32 = 1;
const ENGINE_INFO_SUITE: u32 = 2;
const ENGINE_INFO_PEER_COUNT: u32 = 3;
const ENGINE_INFO_PEER: u32 = 4;
const ENGINE_INFO_FAILURE: u32 = 5;
const ENGINE_INFO_SESSION_ID: u32 = 6;
const ENGINE_INFO_LOCAL_COUNT: u32 = 7;
const ENGINE_INFO_LOCAL: u32 = 8;

const CONTROL_STATE: u32 = 0;
const CONTROL_BEGIN: u32 = 1;
const CONTROL_CLOSE_OUTBOUND: u32 = 2;
const CONTROL_CLOSE_INBOUND: u32 = 3;

enum Context {
  Client(Arc<rustls::ClientConfig>, Option<Arc<Vec<client_identity::Identity>>>),
  Server(Arc<rustls::ServerConfig>),
}

struct OwnedContext {
  config: Context,
  workload: Workload,
}

struct Slot {
  engine: Engine,
  session: Option<[u8; 32]>,
  workload: Workload,
  selected: Option<client_identity::Selected>,
}

type Shard = Mutex<IntMap<u64, Arc<Mutex<Slot>>>>;

const SHARDS: usize = 64;
static CONTEXTS: LazyLock<Mutex<IntMap<u64, Arc<OwnedContext>>>> = LazyLock::new(Mutex::default);
static ENGINES: LazyLock<Box<[Shard]>> = LazyLock::new(|| (0..SHARDS).map(|_| Shard::default()).collect());

/// Identities come from one global sequence, so low bits spread engines across shards.
fn shard(handle: u64) -> &'static Shard {
  &ENGINES[handle as usize % SHARDS]
}

fn engine(handle: u64) -> Option<Arc<Mutex<Slot>>> {
  lock(shard(handle)).get(&handle).cloned()
}

/// # Safety
/// A non-null `data` must be readable for `length` bytes.
unsafe fn bytes<'a>(data: *const u8, length: u64) -> Option<&'a [u8]> {
  let length = usize::try_from(length).ok()?;
  if length == 0 {
    return Some(&[]);
  }
  if data.is_null() {
    return None;
  }
  Some(unsafe { std::slice::from_raw_parts(data, length) })
}

/// # Safety
/// A non-null `data` must be writable for `length` bytes and unaliased for the call.
unsafe fn bytes_mut<'a>(data: *mut u8, length: u64) -> Option<&'a mut [u8]> {
  let length = usize::try_from(length).ok()?;
  if length == 0 {
    return Some(&mut []);
  }
  if data.is_null() {
    return None;
  }
  Some(unsafe { std::slice::from_raw_parts_mut(data, length) })
}

fn protocols(mut bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
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

/// Split concatenated DER `SEQUENCE`s.
fn der_sequence(mut bytes: &[u8]) -> Option<Vec<CertificateDer<'static>>> {
  let mut output = Vec::new();
  while !bytes.is_empty() {
    if bytes.len() < 2 || bytes[0] != 0x30 {
      return None;
    }
    let (header, length) = match bytes[1] {
      length @ 0..=0x7f => (2, length as usize),
      0x81..=0x84 => {
        let count = (bytes[1] & 0x7f) as usize;
        let field = bytes.get(2..2 + count)?;
        (
          2 + count,
          field.iter().fold(0usize, |value, byte| value << 8 | *byte as usize),
        )
      }
      _ => return None,
    };
    let end = header.checked_add(length)?;
    output.push(CertificateDer::from(bytes.get(..end)?.to_vec()));
    bytes = &bytes[end..];
  }
  Some(output)
}

fn certificates(bytes: &[u8], der: bool) -> Option<Vec<CertificateDer<'static>>> {
  let certificates = if der {
    der_sequence(bytes)?
  } else {
    CertificateDer::pem_slice_iter(bytes)
      .collect::<Result<Vec<_>, _>>()
      .ok()?
  };
  (!certificates.is_empty()).then_some(certificates)
}

#[derive(Debug)]
struct InsecureCertificateVerifier(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for InsecureCertificateVerifier {
  fn verify_server_cert(
    &self,
    _end_entity: &CertificateDer<'_>,
    _intermediates: &[CertificateDer<'_>],
    _server_name: &ServerName<'_>,
    _ocsp_response: &[u8],
    _now: rustls::pki_types::UnixTime,
  ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
    Ok(rustls::client::danger::ServerCertVerified::assertion())
  }

  fn verify_tls12_signature(
    &self,
    message: &[u8],
    cert: &CertificateDer<'_>,
    signature: &rustls::DigitallySignedStruct,
  ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls12_signature(message, cert, signature, &self.0.signature_verification_algorithms)
  }

  fn verify_tls13_signature(
    &self,
    message: &[u8],
    cert: &CertificateDer<'_>,
    signature: &rustls::DigitallySignedStruct,
  ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature(message, cert, signature, &self.0.signature_verification_algorithms)
  }

  fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
    self.0.signature_verification_algorithms.supported_schemes()
  }
}

fn context(flags: u32, chain: &[u8], key: &[u8], alpn: &[u8]) -> Option<Context> {
  if (flags & (ENGINE_CLIENT_IDENTITY | ENGINE_CLIENT_IDENTITIES) != 0 && flags & ENGINE_SERVER != 0)
    || flags & (ENGINE_CLIENT_IDENTITY | ENGINE_CLIENT_IDENTITIES)
      == (ENGINE_CLIENT_IDENTITY | ENGINE_CLIENT_IDENTITIES)
  {
    return None;
  }
  let der = flags & ENGINE_DER != 0;
  let mut provider = rustls::crypto::aws_lc_rs::default_provider();
  let tls12 = [&rustls::version::TLS12];
  let tls13 = [&rustls::version::TLS13];
  let (versions, alpn) = if flags & ENGINE_POLICY != 0 {
    let [1, version_mask @ 1..=3, count, rest @ ..] = alpn else {
      return None;
    };
    let (suites, alpn) = rest.split_at_checked(usize::from(*count) * 2)?;
    if *count != 0 {
      let mut selected = Vec::with_capacity(usize::from(*count));
      for code in suites.as_chunks::<2>().0 {
        let code = rustls::CipherSuite::from(u16::from_be_bytes([code[0], code[1]]));
        if selected
          .iter()
          .any(|suite: &rustls::SupportedCipherSuite| suite.suite() == code)
        {
          return None;
        }
        selected.push(*provider.cipher_suites.iter().find(|suite| suite.suite() == code)?);
      }
      provider.cipher_suites = selected;
    }
    let versions: &[&rustls::SupportedProtocolVersion] = match version_mask {
      1 => &tls12,
      2 => &tls13,
      _ => rustls::DEFAULT_VERSIONS,
    };
    (versions, alpn)
  } else {
    (rustls::DEFAULT_VERSIONS, alpn)
  };
  let alpn = protocols(alpn)?;
  let insecure = flags & ENGINE_INSECURE != 0;
  if insecure && flags & ENGINE_SERVER != 0 {
    return None;
  }
  let chain = if insecure {
    Vec::new()
  } else {
    certificates(chain, der)?
  };
  let provider = Arc::new(provider);
  if flags & ENGINE_SERVER != 0 {
    let key = if der {
      PrivateKeyDer::try_from(key.to_vec()).ok()?
    } else {
      PrivateKeyDer::from_pem_slice(key).ok()?
    };
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
      .with_protocol_versions(versions)
      .ok()?
      .with_no_client_auth()
      .with_single_cert(chain, key)
      .ok()?;
    config.alpn_protocols = alpn;
    Some(Context::Server(Arc::new(config)))
  } else {
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
      .with_protocol_versions(versions)
      .ok()?;
    let builder = if insecure {
      builder
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureCertificateVerifier(provider.clone())))
    } else {
      // System bundles can carry anchors webpki cannot parse; skip those rather than fail.
      let mut roots = rustls::RootCertStore::empty();
      roots.add_parsable_certificates(chain);
      if roots.is_empty() {
        return None;
      }
      builder.with_root_certificates(roots)
    };
    let identities = if flags & ENGINE_CLIENT_IDENTITIES != 0 {
      Some(Arc::new(client_identity::parse(key, &provider)?))
    } else if flags & ENGINE_CLIENT_IDENTITY != 0 {
      Some(Arc::new(vec![client_identity::Identity::pem(key, &provider)?]))
    } else {
      if !key.is_empty() {
        return None;
      }
      None
    };
    let mut config = builder.with_no_client_auth();
    // A resumed handshake does not call the resolver; require fresh authentication for identities.
    if identities.is_some() {
      config.resumption = rustls::client::Resumption::disabled();
    }
    config.alpn_protocols = alpn;
    Some(Context::Client(Arc::new(config), identities))
  }
}

/// Build an immutable Rustls/AWS-LC engine context bound to an open workload; zero indicates invalid input.
///
/// Clients take trust anchors in `certificates` unless `ENGINE_INSECURE` is set; servers take their chain and key.
/// `ENGINE_CLIENT_IDENTITY` supplies a PEM client chain and private key together in `key`, independent of `ENGINE_DER`.
/// `alpn` is the wire-format identifier list. Inputs are read during the call only.
///
/// # Safety
/// Each non-null pointer must be readable for its length.
#[allow(clippy::too_many_arguments)] // Preserve the transport ABI call shape.
pub unsafe fn elide_transport_engine_context_new(
  workload: u64,
  flags: u32,
  certificates: *const u8,
  certificates_length: u64,
  key: *const u8,
  key_length: u64,
  alpn: *const u8,
  alpn_length: u64,
) -> u64 {
  let Some(workload) = Workload::admit(workload) else {
    return 0;
  };
  if flags
    & !(ENGINE_SERVER
      | ENGINE_DER
      | ENGINE_INSECURE
      | ENGINE_POLICY
      | ENGINE_CLIENT_IDENTITY
      | ENGINE_CLIENT_IDENTITIES)
    != 0
  {
    return 0;
  }
  let inputs = unsafe {
    (
      bytes(certificates, certificates_length),
      bytes(key, key_length),
      bytes(alpn, alpn_length),
    )
  };
  let (Some(chain), Some(key), Some(alpn)) = inputs else {
    return 0;
  };
  let Some(context) = context(flags, chain, key, alpn) else {
    return 0;
  };
  let id = identity();
  if id != 0 {
    lock(&CONTEXTS).insert(
      id,
      Arc::new(OwnedContext {
        config: context,
        workload,
      }),
    );
  }
  id
}

/// Release a context; engines retain their configuration.
pub fn elide_transport_engine_context_release(context: u64) -> i32 {
  if lock(&CONTEXTS).remove(&context).is_some() {
    0
  } else {
    INVALID
  }
}

/// Create an engine. Clients require a UTF-8 DNS name or IP literal for SNI and verification;
/// servers pass none. The workload must match the context's open owner. Zero indicates invalid input.
///
/// # Safety
/// A non-null `name` must be readable for `name_length` bytes.
pub unsafe fn elide_transport_engine_new(workload: u64, context: u64, name: *const u8, name_length: u64) -> u64 {
  let Some(config) = lock(&CONTEXTS).get(&context).cloned() else {
    return 0;
  };
  if !config.workload.admits(workload) {
    return 0;
  }
  let budget = config.workload.budget.clone();
  let mut selected = None;
  let engine = match &config.config {
    Context::Client(config, identities) => {
      let Some(name) = (unsafe { bytes(name, name_length) }) else {
        return 0;
      };
      let Ok(name) = std::str::from_utf8(name) else {
        return 0;
      };
      let Ok(name) = ServerName::try_from(name.to_owned()) else {
        return 0;
      };
      let config = if let Some(identities) = identities {
        let mut config = (**config).clone();
        let selection = client_identity::Selected::default();
        config.client_auth_cert_resolver = Arc::new(client_identity::Resolver {
          identities: identities.clone(),
          selected: selection.clone(),
        });
        selected = Some(selection);
        Arc::new(config)
      } else {
        config.clone()
      };
      Engine::client_with_budget(config, name, budget)
    }
    Context::Server(config) => Engine::server_with_budget(config.clone(), budget),
  };
  let Ok(engine) = engine else { return 0 };
  let id = identity();
  if id != 0 {
    let slot = Slot {
      engine,
      session: None,
      workload: config.workload.clone(),
      selected,
    };
    lock(shard(id)).insert(id, Arc::new(Mutex::new(slot)));
  }
  id
}

fn flags(engine: &Engine) -> i64 {
  let mut flags = 0;
  if engine.inbound_done() {
    flags |= INBOUND_DONE;
  }
  if engine.outbound_done() {
    flags |= OUTBOUND_DONE;
  }
  if engine.is_handshaking() {
    flags |= HANDSHAKING;
  }
  flags
}

/// Bits 0–23 consumed, 24–47 produced, 48–49 `SSLEngineResult.Status`, 50–52 `HandshakeStatus`,
/// then inbound-done, outbound-done and handshaking flags.
fn pack(engine: &Engine, outcome: Outcome) -> i64 {
  outcome.consumed as i64
    | (outcome.produced as i64) << 24
    | (outcome.status as i64) << 48
    | (outcome.handshake as i64) << 50
    | flags(engine)
}

fn run(handle: u64, operation: impl FnOnce(&mut Engine) -> std::io::Result<Outcome>) -> i64 {
  let Some(slot) = engine(handle) else {
    return INVALID as i64;
  };
  let mut slot = lock(&slot);
  if slot.workload.closed() {
    return ENGINE_FAILED;
  }
  match operation(&mut slot.engine) {
    Ok(outcome) => pack(&slot.engine, outcome),
    Err(_) => ENGINE_FAILED,
  }
}

/// Encrypt up to 16 MiB − 1 of `source` into `destination`, after any pending handshake or
/// alert records. Returns the packed result, `INVALID`, or `ENGINE_FAILED`.
///
/// # Safety
/// `source` must be readable and `destination` writable for their lengths, unaliased, for the call.
pub unsafe fn elide_transport_engine_wrap(
  engine: u64,
  source: *const u8,
  source_length: u64,
  destination: *mut u8,
  destination_length: u64,
) -> i64 {
  let buffers = unsafe {
    (
      bytes(source, source_length.min(LENGTH_MASK)),
      bytes_mut(destination, destination_length.min(LENGTH_MASK)),
    )
  };
  let (Some(source), Some(destination)) = buffers else {
    return INVALID as i64;
  };
  run(engine, |engine| engine.wrap(source, destination))
}

/// Process at most one record from `source` into `destination`. Without `ENGINE_SOURCE_SHARED`
/// the record may be decrypted in place, leaving its consumed bytes undefined.
///
/// # Safety
/// `source` must be readable (writable without the shared flag) and `destination` writable for
/// their lengths, unaliased, for the call.
pub unsafe fn elide_transport_engine_unwrap(
  engine: u64,
  source: *mut u8,
  source_length: u64,
  destination: *mut u8,
  destination_length: u64,
  flags: u32,
) -> i64 {
  let source_length = source_length.min(LENGTH_MASK);
  let destination = unsafe { bytes_mut(destination, destination_length.min(LENGTH_MASK)) };
  let input = if flags & ENGINE_SOURCE_SHARED != 0 {
    unsafe { bytes(source, source_length) }.map(Input::Shared)
  } else {
    unsafe { bytes_mut(source, source_length) }.map(Input::Mutable)
  };
  let (Some(input), Some(destination)) = (input, destination) else {
    return INVALID as i64;
  };
  run(engine, |engine| engine.unwrap(input, destination))
}

/// Operations: 0 state, 1 begin handshake, 2 close outbound, 3 close inbound (bit 56 set when
/// the peer's close-notify is missing). Returns packed flags with the handshake status.
pub fn elide_transport_engine_control(engine: u64, operation: u32) -> i64 {
  let Some(slot) = self::engine(engine) else {
    return INVALID as i64;
  };
  let mut slot = lock(&slot);
  if operation == CONTROL_BEGIN && slot.workload.closed() {
    return ENGINE_FAILED;
  }
  let engine = &mut slot.engine;
  let truncated = match operation {
    CONTROL_STATE => false,
    CONTROL_BEGIN => {
      if engine.begin().is_err() {
        return ENGINE_FAILED;
      }
      false
    }
    CONTROL_CLOSE_OUTBOUND => {
      engine.close_outbound();
      false
    }
    CONTROL_CLOSE_INBOUND => engine.close_inbound(),
    _ => return INVALID as i64,
  };
  let status = if engine.outbound_done() {
    Status::Closed
  } else {
    Status::Ok
  };
  let mut packed = (status as i64) << 48 | (engine.handshake_status() as i64) << 50 | flags(engine);
  if truncated {
    packed |= TRUNCATED;
  }
  packed
}

/// Session details: 0 ALPN, 1 protocol code, 2 cipher suite code, 3 peer certificate count,
/// 4 peer certificate DER at `index`, 5 failure text, 6 32-byte session identifier. Byte kinds
/// copy up to `capacity` bytes and return the full length; numeric kinds return the value, or zero
/// when not negotiated.
///
/// # Safety
/// A non-null `output` must be writable for `capacity` bytes.
pub unsafe fn elide_transport_engine_info(engine: u64, kind: u32, index: u32, output: *mut u8, capacity: u64) -> i32 {
  let Some(output) = (unsafe { bytes_mut(output, capacity) }) else {
    return INVALID;
  };
  let Some(slot) = self::engine(engine) else {
    return INVALID;
  };
  let mut slot = lock(&slot);
  let copy = |bytes: &[u8], output: &mut [u8]| {
    let count = bytes.len().min(output.len());
    output[..count].copy_from_slice(&bytes[..count]);
    i32::try_from(bytes.len()).unwrap_or(INVALID)
  };
  match kind {
    ENGINE_INFO_ALPN => copy(slot.engine.alpn_protocol().unwrap_or_default(), output),
    ENGINE_INFO_PROTOCOL => slot.engine.protocol_version().map_or(0, i32::from),
    ENGINE_INFO_SUITE => slot.engine.cipher_suite().map_or(0, i32::from),
    ENGINE_INFO_PEER_COUNT => slot.engine.peer_certificates().len() as i32,
    ENGINE_INFO_PEER => match slot.engine.peer_certificates().get(index as usize) {
      Some(certificate) => copy(certificate.as_ref(), output),
      None => INVALID,
    },
    ENGINE_INFO_LOCAL_COUNT => slot.selected.as_ref().map_or(0, |selected| {
      lock(selected).as_ref().map_or(0, |key| key.cert.len() as i32)
    }),
    ENGINE_INFO_LOCAL => {
      let Some(selected) = slot.selected.as_ref() else {
        return INVALID;
      };
      match lock(selected).as_ref().and_then(|key| key.cert.get(index as usize)) {
        Some(certificate) => copy(certificate.as_ref(), output),
        None => INVALID,
      }
    }
    ENGINE_INFO_FAILURE => copy(
      if slot.workload.closed() {
        b"TLS workload is closed"
      } else {
        slot.engine.failure().unwrap_or_default().as_bytes()
      },
      output,
    ),
    ENGINE_INFO_SESSION_ID => {
      if slot.session.is_none() {
        let mut id = [0; 32];
        if aws_lc_rs::rand::fill(&mut id).is_err() {
          return INVALID;
        }
        slot.session = Some(id);
      }
      copy(&slot.session.unwrap_or_default(), output)
    }
    _ => INVALID,
  }
}

/// Release an engine from any thread once no call is in progress.
pub fn elide_transport_engine_release(engine: u64) -> i32 {
  if lock(shard(engine)).remove(&engine).is_some() {
    0
  } else {
    INVALID
  }
}
