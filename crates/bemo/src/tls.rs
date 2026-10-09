/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Driver-independent Rustls state machine with caller-owned native output.

use std::io;
use std::sync::Arc;

use rustls::client::UnbufferedClientConnection;
use rustls::pki_types::ServerName;
use rustls::server::UnbufferedServerConnection;
use rustls::unbuffered::{ConnectionState, EncodeError, EncryptError, UnbufferedStatus};

use crate::buffer::{Budget, Buffer, FrozenBuffer, TlsOutput};

pub mod engine;

const MAX_OUTPUT: usize = 1024 * 1024;
/// Bound application work per transition while allowing Rustls to fragment records internally.
pub const MAX_WRITE: usize = 128 * 1024;

/// Work requested at the next TLS transition. Unaccepted writes remain owned by the caller.
#[derive(Clone, Copy)]
pub enum Action<'a> {
  /// Process incoming records or handshake progress.
  Continue,
  /// All previously encoded handshake bytes have completed transport writes.
  Transmitted,
  /// Encrypt up to MAX_WRITE plaintext bytes as an ordered record batch when the handshake permits it.
  Write(&'a [u8]),
  /// Queue close-notify when application writes are permitted.
  Close,
}

/// Resulting TLS state, independent of the socket driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
  /// More incoming TLS bytes are needed.
  NeedRead,
  /// Handshake bytes have been encoded and must be queued for transport.
  Encoded,
  /// All encoded handshake bytes must be transmitted before acknowledgement.
  NeedTransmit,
  /// The handshake allows application traffic.
  Ready,
  /// Authenticated plaintext is available.
  Plaintext,
  /// Peer close-notify received.
  PeerClosed,
  /// Both TLS directions are closed.
  Closed,
}

/// One bounded state-machine transition. Discard input bytes before invoking the next step.
pub struct Step {
  /// Bytes consumed from the front of input.
  pub discard: usize,
  /// Resulting state.
  pub state: State,
  /// Plaintext bytes accepted from a Write action.
  pub accepted: usize,
  /// Encoded TLS storage to send in order.
  pub output: Option<FrozenBuffer>,
  /// Authenticated data retaining its own native allocation.
  pub plaintext: Option<FrozenBuffer>,
}

enum Inner {
  Client(UnbufferedClientConnection),
  Server(UnbufferedServerConnection),
}

/// Thread-owned TLS session. Its protocol state performs no socket I/O or runtime scheduling.
pub struct Session {
  inner: Inner,
  budget: Budget,
  failed: bool,
}

impl Session {
  /// Create a verified client session using an explicit Rustls configuration and server name.
  ///
  /// # Errors
  /// Rejects early-data configuration and invalid Rustls settings.
  pub fn client(config: Arc<rustls::ClientConfig>, name: ServerName<'static>, budget: Budget) -> io::Result<Self> {
    if config.enable_early_data {
      return Err(io::ErrorKind::Unsupported.into());
    }
    Ok(Self {
      inner: Inner::Client(UnbufferedClientConnection::new(config, name).map_err(tls_error)?),
      budget,
      failed: false,
    })
  }

  /// Create a server session using explicit identity and verification configuration.
  ///
  /// # Errors
  /// Rejects early data and invalid Rustls settings.
  pub fn server(config: Arc<rustls::ServerConfig>, budget: Budget) -> io::Result<Self> {
    if config.max_early_data_size != 0 {
      return Err(io::ErrorKind::Unsupported.into());
    }
    Ok(Self {
      inner: Inner::Server(UnbufferedServerConnection::new(config).map_err(tls_error)?),
      budget,
      failed: false,
    })
  }

  /// Whether peer authentication and handshake processing are still in progress.
  pub fn is_handshaking(&self) -> bool {
    match &self.inner {
      Inner::Client(c) => c.is_handshaking(),
      Inner::Server(c) => c.is_handshaking(),
    }
  }

  /// Negotiated application protocol, if any.
  pub fn alpn_protocol(&self) -> Option<&[u8]> {
    match &self.inner {
      Inner::Client(c) => c.alpn_protocol(),
      Inner::Server(c) => c.alpn_protocol(),
    }
  }

  /// Advance one transition. The caller must remove `Step::discard` bytes before the next call,
  /// preserve output order, and acknowledge transmission only after transport completion.
  ///
  /// # Errors
  /// Returns TLS, allocation, or unsupported-state errors; any error makes the session terminal.
  pub fn step(&mut self, input: &mut [u8], action: Action<'_>) -> io::Result<Step> {
    if self.failed {
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    let result = match &mut self.inner {
      Inner::Client(c) => advance(c.process_tls_records(input), action, &self.budget),
      Inner::Server(c) => advance(c.process_tls_records(input), action, &self.budget),
    };
    self.failed = result.is_err();
    result
  }
}

fn advance<Data>(status: UnbufferedStatus<'_, '_, Data>, action: Action<'_>, budget: &Budget) -> io::Result<Step> {
  let mut step = Step {
    discard: status.discard,
    state: State::NeedRead,
    accepted: 0,
    output: None,
    plaintext: None,
  };
  match status.state.map_err(tls_error)? {
    ConnectionState::BlockedHandshake => {}
    ConnectionState::EncodeTlsData(mut encode) => {
      let size = match encode.encode(&mut []) {
        Err(EncodeError::InsufficientSize(size)) if size.required_size <= MAX_OUTPUT => size.required_size,
        Err(error) => return Err(tls_error(error)),
        Ok(_) => {
          step.state = State::Encoded;
          return Ok(step);
        }
      };
      let mut buffer = TlsOutput::new(size, budget.clone())?;
      let length = encode.encode(buffer.as_mut()).map_err(tls_error)?;
      step.output = Some(buffer.freeze(length));
      step.state = State::Encoded;
    }
    ConnectionState::TransmitTlsData(transmit) => {
      if matches!(action, Action::Transmitted) {
        transmit.done();
      }
      step.state = State::NeedTransmit;
    }
    ConnectionState::ReadTraffic(mut traffic) => {
      if let Some(size) = traffic.peek_len() {
        let mut buffer = Buffer::new(size.get(), budget.clone())?;
        if let Some(record) = traffic.next_record() {
          let record = record.map_err(tls_error)?;
          // Rustls 0.23.43 retains plaintext internally; this is the explicit TLS staging copy.
          buffer.write(0, record.payload)?;
          step.discard += record.discard;
          step.plaintext = Some(buffer.freeze());
        }
      }
      step.state = State::Plaintext;
    }
    ConnectionState::WriteTraffic(mut traffic) => {
      step.state = State::Ready;
      let mut write_length = match action {
        Action::Write(bytes) => bytes.len().min(MAX_WRITE),
        _ => 0,
      };
      if matches!(action, Action::Write(bytes) if !bytes.is_empty()) || matches!(action, Action::Close) {
        let mut buffer: Option<TlsOutput> = None;
        let mut capacity = 0;
        loop {
          let output = buffer.as_mut().map_or(&mut [][..], TlsOutput::as_mut);
          let encrypted = match action {
            Action::Write(bytes) => traffic.encrypt(&bytes[..write_length], output),
            Action::Close => traffic.queue_close_notify(output),
            _ => unreachable!(),
          };
          match encrypted {
            Ok(length) => {
              if let Some(buffer) = buffer {
                step.output = Some(buffer.freeze(length));
              }
              break;
            }
            Err(EncryptError::InsufficientSize(size))
              if size.required_size > capacity && size.required_size <= MAX_OUTPUT =>
            {
              // A size probe does not encrypt payload, but may queue a key update before retry.
              capacity = size.required_size;
              drop(buffer.take());
              match TlsOutput::new(capacity, budget.clone()) {
                Ok(storage) => buffer = Some(storage),
                Err(error) if error.kind() == io::ErrorKind::OutOfMemory && write_length > 16384 => {
                  // Preserve the single-record path for owners whose remaining budget cannot
                  // afford a batch. The failed size probe consumed no application bytes.
                  write_length = (write_length / 2).max(16384);
                  capacity = 0;
                }
                Err(error) => return Err(error),
              }
            }
            Err(error) => return Err(tls_error(error)),
          }
        }
        if let Action::Write(_) = action {
          step.accepted = write_length;
        }
      }
    }
    ConnectionState::PeerClosed => step.state = State::PeerClosed,
    ConnectionState::Closed => step.state = State::Closed,
    _ => return Err(io::ErrorKind::Unsupported.into()),
  }
  Ok(step)
}

fn tls_error(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, error)
}
