/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Drivers for crate-private protocol state, compiled only with the `fuzzing` feature.
//!
//! These are not API: they exist so the separate fuzz workspace can reach state
//! machines that embedders only use through `abi`. Each driver asserts the
//! invariants the ABI layer relies on, so a violated invariant is a crash.

use crate::http::h2::{Connection, Event, MAX_STREAMS};
use bytes::Bytes;
use h2::Reason;
use http::Response;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::task::{Context, Waker};

/// The client connection preface; prepended so mutations reach frame parsing.
const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// Upper bound on driver rounds, keeping every input's execution finite.
const ROUNDS: usize = 512;

#[derive(Default)]
struct Stream {
  /// Received DATA bytes not yet released to the peer's flow-control window.
  unacked: usize,
  responded: bool,
  ended: bool,
  reset: bool,
}

/// Feed arbitrary client frames to the native HTTP/2 server while acting as a
/// well-behaved application: respond, send bounded DATA, acknowledge received
/// bytes, and release streams. The first two bytes select fragmentation and
/// application behavior; the rest follows the client preface on the wire.
pub fn h2_server(data: &[u8]) {
  let [chunk, behavior, wire @ ..] = data else {
    return;
  };
  let chunk = usize::from(*chunk) * 16 + 1;
  let behavior = *behavior;
  let mut input = PREFACE.to_vec();
  input.extend_from_slice(wire);
  let mut connection = Connection::new();
  let mut cx = Context::from_waker(Waker::noop());
  let mut streams = BTreeMap::<u32, Stream>::new();
  let mut offset = 0;
  let mut output = 0usize;
  let mut wire_end = 0u64;
  let mut eof = false;
  for round in 0..ROUNDS {
    if offset < input.len() {
      let end = (offset + chunk)
        .min(input.len())
        .min(offset + connection.input_capacity());
      if end > offset {
        match connection.feed(Bytes::copy_from_slice(&input[offset..end])) {
          Ok(()) => offset = end,
          Err(error) => assert_eq!(error.kind(), io::ErrorKind::WouldBlock, "feed failed: {error}"),
        }
      }
    } else if !eof {
      // Peer half-close after the last byte; also start a graceful drain on some inputs.
      if behavior & 0x40 != 0 {
        connection.drain();
      }
      connection.eof();
      eof = true;
    }
    let events = match connection.poll(&mut cx, 64) {
      Ok(events) => events,
      Err(_) => {
        assert!(connection.is_closed(), "poll errors must close the connection");
        break;
      }
    };
    // One batch is collected before the application acts, so later events in it may name a
    // stream the application released while handling an earlier one (the ABI does this).
    let mut released = BTreeSet::new();
    for event in events {
      handle(&mut connection, &mut streams, &mut released, event, behavior, round);
    }
    // Drain a bounded amount of output per round, as a socket with backpressure would.
    let limit = 1usize << (8 + (behavior & 0x7));
    while let Some(bytes) = connection.take_output_limit(limit) {
      assert!(bytes.len() <= limit, "output exceeded the requested limit");
      output += bytes.len();
    }
    for mark in connection.take_frames() {
      // Marks may describe bytes taken in a later chunk, but wire positions only advance.
      assert!(mark.wire_end >= wire_end, "frame marks moved backwards");
      wire_end = mark.wire_end;
    }
    if connection.is_closed() {
      break;
    }
  }
  assert!(
    streams.len() <= MAX_STREAMS,
    "accepted more than the advertised stream limit"
  );
  // Releasing every stream must cancel incomplete exchanges without panicking.
  for id in streams.into_keys() {
    connection.release(id);
  }
  connection.finish_shutdown();
  let _ = connection.poll(&mut cx, 64);
  while let Some(bytes) = connection.take_output() {
    output += bytes.len();
  }
  for mark in connection.take_frames() {
    assert!(mark.wire_end >= wire_end, "frame marks moved backwards");
    wire_end = mark.wire_end;
  }
  // Once every output byte is taken, no mark may describe a byte that was never produced.
  assert!(wire_end <= output as u64, "frame mark beyond produced output");
}

fn handle(
  connection: &mut Connection,
  streams: &mut BTreeMap<u32, Stream>,
  released: &mut BTreeSet<u32>,
  event: Event,
  behavior: u8,
  round: usize,
) {
  match &event {
    Event::Data { stream, .. } | Event::End { stream } | Event::Reset { stream, .. } | Event::Writable { stream }
      if released.contains(stream) =>
    {
      return;
    }
    _ => {}
  }
  match event {
    Event::Request { stream: id, end, .. } => {
      assert!(id % 2 == 1, "client-initiated streams are odd");
      assert!(
        streams
          .insert(
            id,
            Stream {
              ended: end,
              ..Stream::default()
            }
          )
          .is_none(),
        "duplicate request event"
      );
      if behavior & 0x08 != 0 {
        respond(connection, streams, released, id, behavior, round);
      }
    }
    Event::Data { stream: id, bytes } => {
      let stream = streams.get_mut(&id).expect("DATA for an unannounced stream");
      assert!(!stream.ended, "DATA after END_STREAM");
      stream.unacked += bytes.len();
      // Over-acknowledgement must be refused without changing state.
      assert!(connection.ack(id, stream.unacked + 1).is_err());
      if behavior & 0x10 != 0 {
        connection
          .ack(id, stream.unacked)
          .expect("acknowledging received bytes");
        stream.unacked = 0;
      }
    }
    Event::End { stream: id } => {
      let stream = streams.get_mut(&id).expect("END for an unannounced stream");
      assert!(!stream.ended, "duplicate END");
      stream.ended = true;
      if !stream.responded {
        respond(connection, streams, released, id, behavior, round);
      }
    }
    Event::Reset { stream: id, .. } => {
      let stream = streams.get_mut(&id).expect("RST for an unannounced stream");
      assert!(!stream.reset, "duplicate reset event");
      stream.reset = true;
      assert!(
        connection.data(id, Bytes::from_static(b"x"), true).is_err(),
        "DATA accepted after reset"
      );
      streams.remove(&id);
      connection.release(id);
      released.insert(id);
    }
    Event::Writable { stream: id } => {
      assert!(streams.contains_key(&id), "Writable for an unannounced stream");
      let _ = connection.data(id, Bytes::from(vec![0; 1024]), true);
    }
  }
}

fn respond(
  connection: &mut Connection,
  streams: &mut BTreeMap<u32, Stream>,
  released: &mut BTreeSet<u32>,
  id: u32,
  behavior: u8,
  round: usize,
) {
  let stream = streams.get_mut(&id).expect("response for an unannounced stream");
  if behavior & 0x20 != 0 && round.is_multiple_of(3) {
    connection.reset(id, Reason::REFUSED_STREAM);
    streams.remove(&id);
    connection.release(id);
    released.insert(id);
    return;
  }
  let body = usize::from(behavior >> 6) * 24 * 1024;
  let head = Response::builder().status(200).body(()).expect("static response head");
  if connection.response(id, head, body == 0).is_err() {
    return;
  }
  stream.responded = true;
  if body != 0 {
    // A partial accept is valid; the remainder waits for Writable.
    let accepted = connection.data(id, Bytes::from(vec![b'b'; body]), true).unwrap_or(0);
    assert!(accepted <= body);
  }
  assert!(
    connection.response(id, Response::new(()), true).is_err(),
    "a second final response was accepted"
  );
}
