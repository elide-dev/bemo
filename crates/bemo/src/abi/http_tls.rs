/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! TLS integration with the HTTP driver's single ordered write lane.

use super::*;
use crate::http::tls::{Lane, Progress};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct TlsSocket {
  pub(super) lane: Lane,
  pub(super) driving: bool,
  pub(super) input_eof: bool,
  wire: Vec<FrozenBuffer>,
  wire_sending: bool,
  needs_drive: bool,
  plaintext: VecDeque<FrozenBuffer>,
  started: Instant,
  input_ended: bool,
}

impl TlsSocket {
  pub(super) fn new(config: Arc<rustls::ServerConfig>, budget: Budget) -> io::Result<Self> {
    Ok(Self {
      lane: Lane::new(config, budget)?,
      driving: false,
      input_eof: false,
      wire: Vec::new(),
      wire_sending: false,
      needs_drive: false,
      plaintext: VecDeque::new(),
      started: Instant::now(),
      input_ended: false,
    })
  }

  pub(super) fn needs_poll(&self) -> bool {
    self.needs_drive
  }
}

pub(super) fn end_input(state: &mut DriverState, socket: u64, events: &mut VecDeque<NativeEvent>) {
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let Some(tls) = &mut http.tls else {
    return;
  };
  tls.input_eof = true;
  if let Some(id) = http.persistent {
    let _ = state.driver.pause_persistent_receive(id);
  }
  drive(state, socket, events);
  pump(state, socket, events);
}

pub(super) fn on_sent(
  state: &mut DriverState,
  socket: u64,
  status: io::Result<usize>,
  mut buffers: Vec<FrozenBuffer>,
  events: &mut VecDeque<NativeEvent>,
) {
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let Some(tls) = &mut http.tls else {
    return;
  };
  tls.wire_sending = false;
  match status.and_then(|sent| buffers.advance(sent)) {
    Ok(true) => {
      tls.lane.transmitted();
      http.sending = tls.lane.has_application();
    }
    Ok(false) => tls.wire = buffers,
    Err(error) => {
      fail_writes(http, socket, error_code(error), events);
      close_socket(state, socket, events);
      return;
    }
  }
  drive(state, socket, events);
  pump(state, socket, events);
}

pub(super) fn drive(state: &mut DriverState, socket: u64, events: &mut VecDeque<NativeEvent>) {
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let Some(tls) = &mut http.tls else {
    return;
  };
  if tls.driving {
    return;
  }
  if tls.lane.is_handshaking() && (http.draining || tls.started.elapsed() >= Duration::from_secs(10)) {
    close_socket(state, socket, events);
    return;
  }
  tls.driving = true;
  tls.needs_drive = false;
  let mut completed = false;
  let mut blocked = false;
  let mut inline_bytes = 0;
  for _ in 0..256 {
    let Some(http) = state.http.sockets.get_mut(&socket) else {
      return;
    };
    let tls = http.tls.as_mut().unwrap();
    let plaintext_fits = http.h2.as_ref().is_none_or(|h2| {
      tls
        .plaintext
        .front()
        .is_none_or(|bytes| bytes.as_ref().len() <= h2.input_capacity())
    });
    if plaintext_fits
      && (http.h2.is_some()
        || (http.pending_input.is_empty()
          && (http.outstanding < MAX_EXCHANGES || http.parser.body_pending())
          && http.pinned.get() < BODY_WINDOW_BYTES))
      && let Some(plaintext) = tls.plaintext.pop_front()
    {
      if tls.lane.alpn_protocol() == Some(b"h2".as_slice()) {
        h2_socket::feed(state, socket, plaintext, events);
      } else {
        process_input(state, socket, plaintext, events);
      }
      continue;
    }
    if tls.wire_sending {
      break;
    }
    if !tls.wire.is_empty() {
      let Some(connection) = state.sockets.get(&socket) else {
        break;
      };
      // Polling sockets can accept ciphertext synchronously. Keep the same immutable
      // lease and acknowledge a TLS transmission only once its complete record is sent.
      // Backpressure and completion-only backends retain the asynchronous write lane.
      match state.driver.try_send(connection, tls.wire[0].as_ref()) {
        Ok(Some(sent)) => {
          match tls.wire.advance(sent) {
            Ok(true) => {
              tls.lane.transmitted();
              http.sending = tls.lane.has_application();
            }
            Ok(false) => {}
            Err(error) => {
              fail_writes(http, socket, error_code(error), events);
              close_socket(state, socket, events);
              return;
            }
          }
          inline_bytes += sent;
          if inline_bytes >= 128 * 1024 {
            break;
          }
          continue;
        }
        Ok(None) => {}
        Err(error) => {
          fail_writes(http, socket, error_code(error), events);
          close_socket(state, socket, events);
          return;
        }
      }
      let wire = std::mem::take(&mut tls.wire);
      match state.driver.send_vectored(connection, wire) {
        Ok(operation) => {
          state.operations.insert(operation, Operation::http(socket));
          tls.wire_sending = true;
          http.sending = true;
        }
        Err((error, wire)) if error.kind() == io::ErrorKind::WouldBlock => {
          tls.wire = wire;
          state.http.queue_retry(socket);
        }
        Err((error, _)) => {
          fail_writes(http, socket, error_code(error), events);
          close_socket(state, socket, events);
          return;
        }
      }
      break;
    }
    match tls.lane.progress() {
      Ok(Progress::Output(output)) => tls.wire.push(output),
      Ok(Progress::Plaintext(plaintext)) => tls.plaintext.push_back(plaintext.freeze()),
      Ok(Progress::Complete(parts)) => {
        let length = parts.iter().map(|part| part.as_ref().len()).sum();
        on_plain_sent(state, socket, Ok(length), parts, events);
        completed = true;
      }
      Ok(Progress::InputClosed) => {}
      Ok(Progress::Blocked) => {
        blocked = true;
        break;
      }
      Ok(Progress::Closed) => {
        close_socket(state, socket, events);
        return;
      }
      Err(error) => {
        fail_writes(http, socket, error_code(error), events);
        close_socket(state, socket, events);
        return;
      }
    }
  }
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let tls = http.tls.as_mut().unwrap();
  tls.driving = false;
  let yielded = !blocked && !tls.wire_sending && (!tls.wire.is_empty() || tls.lane.has_application());
  tls.needs_drive = yielded;
  // Multishot EOF may follow ciphertext before the handshake write completes. Drain accepted
  // ciphertext/plaintext first; only close_notify permits preserving delayed responses.
  if blocked && tls.input_eof && !tls.lane.input_closed() && tls.plaintext.is_empty() && http.pending_input.is_empty() {
    close_socket(state, socket, events);
    return;
  }
  let ended = tls.lane.input_closed() && !tls.input_ended && tls.plaintext.is_empty() && http.pending_input.is_empty();
  if ended {
    tls.input_ended = true;
    // close_notify ends input, not output. Preserve accepted responses and finish any partial body
    // with an error so a handler waiting on its body can settle before the output lane closes.
    if http.h2.is_some() {
      h2_socket::peer_closed(state, socket, events);
    } else {
      http.closing = true;
      discard_body(http, socket, http.body, events);
    }
  }
  if yielded {
    // Inline records can exhaust the bounded drive loop without producing a CQE.
    // Keep remaining work runnable rather than waiting for unrelated socket input.
    state.http.queue_retry(socket);
  }
  if completed || ended {
    pump(state, socket, events);
  }
}
