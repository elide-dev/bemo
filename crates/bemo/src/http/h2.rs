/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! HTTP/2 polled by the transport driver, without a Tokio runtime or socket ownership.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use bytes::{Buf, Bytes};
use h2::server::{Handshake, SendResponse};
use h2::{Reason, RecvStream, SendStream};
use http::{Request, Response};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(crate) const MAX_STREAMS: usize = 64;
pub(crate) const WINDOW: usize = 256 * 1024;
const HEAD_LIMIT: u32 = 16 * 1024;
const MARK_LIMIT: usize = WINDOW / 9 + 1;

/// Outgoing frame boundaries produced by h2, measured in plaintext HTTP/2 wire bytes.
/// A mark becomes a write completion only after the transport physically sends `wire_end`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameMark {
  pub(crate) wire_end: u64,
  pub(crate) stream: u32,
  pub(crate) payload_bytes: usize,
  pub(crate) kind: u8,
  pub(crate) flags: u8,
}

pub(crate) const DATA: u8 = 0;
pub(crate) const HEADERS: u8 = 1;
pub(crate) const CONTINUATION: u8 = 9;
pub(crate) const END_STREAM: u8 = 1;
pub(crate) const END_HEADERS: u8 = 4;

#[derive(Default)]
struct FrameTracker {
  header: [u8; 9],
  header_bytes: usize,
  remaining: usize,
  position: u64,
  marks: VecDeque<FrameMark>,
}

impl FrameTracker {
  fn record(&mut self, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
      if self.header_bytes < 9 {
        let n = bytes.len().min(9 - self.header_bytes);
        self.header[self.header_bytes..self.header_bytes + n].copy_from_slice(&bytes[..n]);
        self.header_bytes += n;
        self.position += n as u64;
        bytes = &bytes[n..];
        if self.header_bytes < 9 {
          break;
        }
        self.remaining = self.length();
        // h2 never emits padded DATA; payload credit must not silently count padding bytes.
        if self.header[3] == DATA && self.header[4] & 8 != 0 {
          return Err(io::ErrorKind::InvalidData.into());
        }
      }
      let n = bytes.len().min(self.remaining);
      self.remaining -= n;
      self.position += n as u64;
      bytes = &bytes[n..];
      if self.remaining == 0 {
        let kind = self.header[3];
        if matches!(kind, DATA | HEADERS | CONTINUATION) {
          self.marks.push_back(FrameMark {
            wire_end: self.position,
            stream: u32::from_be_bytes(self.header[5..9].try_into().unwrap()) & 0x7fff_ffff,
            payload_bytes: self.length(),
            kind,
            flags: self.header[4],
          });
        }
        self.header_bytes = 0;
      }
    }
    Ok(())
  }

  fn length(&self) -> usize {
    ((self.header[0] as usize) << 16) | ((self.header[1] as usize) << 8) | self.header[2] as usize
  }
}

#[derive(Default)]
struct Wire {
  input: VecDeque<Bytes>,
  output: VecDeque<Bytes>,
  input_bytes: usize,
  output_bytes: usize,
  eof: bool,
  reader: Option<Waker>,
  writer: Option<Waker>,
  frames: FrameTracker,
}

#[derive(Clone, Default)]
struct MemoryIo(Rc<RefCell<Wire>>);

impl AsyncRead for MemoryIo {
  fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
    let mut wire = self.0.borrow_mut();
    if buf.remaining() == 0 {
      return Poll::Ready(Ok(()));
    }
    if let Some(front) = wire.input.front_mut() {
      let n = front.len().min(buf.remaining());
      buf.put_slice(&front[..n]);
      front.advance(n);
      if front.is_empty() {
        wire.input.pop_front();
      }
      wire.input_bytes -= n;
      return Poll::Ready(Ok(()));
    }
    if wire.eof {
      return Poll::Ready(Ok(()));
    }
    wire.reader = Some(cx.waker().clone());
    Poll::Pending
  }
}

impl AsyncWrite for MemoryIo {
  fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
    let mut wire = self.0.borrow_mut();
    let marker_room = (MARK_LIMIT - wire.frames.marks.len())
      .saturating_mul(9)
      .saturating_sub(8);
    let n = bytes.len().min(WINDOW - wire.output_bytes).min(marker_room);
    if n == 0 && !bytes.is_empty() {
      wire.writer = Some(cx.waker().clone());
      return Poll::Pending;
    }
    if n != 0 {
      wire.frames.record(&bytes[..n])?;
      wire.output.push_back(Bytes::copy_from_slice(&bytes[..n]));
      wire.output_bytes += n;
    }
    Poll::Ready(Ok(n))
  }

  fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }

  fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }
}

impl MemoryIo {
  fn feed(&self, bytes: Bytes) -> io::Result<()> {
    let wake = {
      let mut wire = self.0.borrow_mut();
      if wire.eof {
        return Err(io::ErrorKind::BrokenPipe.into());
      }
      if bytes.len() > WINDOW - wire.input_bytes {
        return Err(io::ErrorKind::WouldBlock.into());
      }
      if bytes.is_empty() {
        return Ok(());
      }
      wire.input_bytes += bytes.len();
      wire.input.push_back(bytes);
      wire.reader.take()
    };
    if let Some(wake) = wake {
      wake.wake();
    }
    Ok(())
  }

  #[cfg(test)]
  fn output(&self) -> Option<Bytes> {
    self.output_limit(usize::MAX)
  }

  fn output_limit(&self, limit: usize) -> Option<Bytes> {
    if limit == 0 {
      return None;
    }
    let (bytes, wake) = {
      let mut wire = self.0.borrow_mut();
      let front = wire.output.front_mut()?;
      let bytes = front.split_to(limit.min(front.len()));
      if front.is_empty() {
        wire.output.pop_front();
      }
      wire.output_bytes -= bytes.len();
      (bytes, wire.writer.take())
    };
    if let Some(wake) = wake {
      wake.wake();
    }
    Some(bytes)
  }
}

pub(crate) enum Event {
  Request {
    stream: u32,
    head: Request<()>,
    end: bool,
  },
  Data {
    stream: u32,
    bytes: Bytes,
  },
  End {
    stream: u32,
  },
  Reset {
    stream: u32,
    reason: Reason,
  },
  /// DATA may be accepted now; this is not a socket-write completion.
  Writable {
    stream: u32,
  },
}

struct Stream {
  recv: RecvStream,
  respond: SendResponse<Bytes>,
  send: Option<SendStream<Bytes>>,
  received_end: bool,
  sent_end: bool,
  reset: bool,
  awaiting_capacity: bool,
  unacked: usize,
}

enum State {
  Handshake(Pin<Box<Handshake<MemoryIo, Bytes>>>),
  Connected(Box<h2::server::Connection<MemoryIo, Bytes>>),
  Closed,
}

pub(crate) struct Connection {
  io: MemoryIo,
  state: State,
  streams: BTreeMap<u32, Stream>,
  draining: bool,
}

impl Connection {
  pub(crate) fn new() -> Self {
    let io = MemoryIo::default();
    let mut builder = h2::server::Builder::new();
    builder
      .max_concurrent_streams(MAX_STREAMS as u32)
      .max_header_list_size(HEAD_LIMIT)
      .initial_window_size(WINDOW as u32)
      .initial_connection_window_size(WINDOW as u32)
      .max_send_buffer_size(WINDOW)
      .max_pending_accept_reset_streams(MAX_STREAMS)
      .max_concurrent_reset_streams(MAX_STREAMS);
    Self {
      state: State::Handshake(Box::pin(builder.handshake(io.clone()))),
      io,
      streams: BTreeMap::new(),
      draining: false,
    }
  }

  /// Feed authenticated or cleartext HTTP/2 wire bytes; retain the caller's bytes on WouldBlock.
  pub(crate) fn feed(&mut self, bytes: Bytes) -> io::Result<()> {
    self.io.feed(bytes)
  }

  pub(crate) fn input_capacity(&self) -> usize {
    WINDOW - self.io.0.borrow().input_bytes
  }

  /// Output ownership transfers to the driver, which must independently track physical sends.
  #[cfg(test)]
  pub(crate) fn take_output(&mut self) -> Option<Bytes> {
    self.io.output()
  }

  pub(crate) fn take_output_limit(&mut self, limit: usize) -> Option<Bytes> {
    self.io.output_limit(limit)
  }

  /// Drain metadata together with output. Mark offsets may cover bytes in later output chunks.
  pub(crate) fn take_frames(&mut self) -> Vec<FrameMark> {
    let (marks, wake) = {
      let mut wire = self.io.0.borrow_mut();
      (wire.frames.marks.drain(..).collect(), wire.writer.take())
    };
    if let Some(wake) = wake {
      wake.wake();
    }
    marks
  }

  #[cfg(test)]
  pub(crate) fn eof(&mut self) {
    let wake = {
      let mut wire = self.io.0.borrow_mut();
      wire.eof = true;
      wire.reader.take()
    };
    if let Some(wake) = wake {
      wake.wake();
    }
  }

  pub(crate) fn is_closed(&self) -> bool {
    matches!(self.state, State::Closed)
  }

  /// Drive protocol work and emit no more than `limit` events. Poll again after feed, write drain,
  /// acknowledgements, response changes, or a wake from `cx`; no executor is installed here.
  pub(crate) fn poll(&mut self, cx: &mut Context<'_>, limit: usize) -> io::Result<Vec<Event>> {
    let mut events = Vec::new();
    if limit == 0 {
      return Ok(events);
    }
    if let State::Handshake(handshake) = &mut self.state {
      match handshake.as_mut().poll(cx) {
        Poll::Pending => return Ok(events),
        Poll::Ready(Ok(mut connection)) => {
          if self.draining {
            connection.graceful_shutdown();
          }
          self.state = State::Connected(Box::new(connection));
        }
        Poll::Ready(Err(error)) => {
          self.state = State::Closed;
          return Err(io::Error::other(error));
        }
      }
    }
    if let State::Connected(connection) = &mut self.state {
      while self.streams.len() < MAX_STREAMS && events.len() < limit {
        match connection.poll_accept(cx) {
          Poll::Ready(Some(Ok((request, respond)))) => {
            let id = respond.stream_id().as_u32();
            let (head, recv) = request.into_parts();
            let end = recv.is_end_stream();
            self.streams.insert(
              id,
              Stream {
                recv,
                respond,
                send: None,
                received_end: end,
                sent_end: false,
                reset: false,
                awaiting_capacity: false,
                unacked: 0,
              },
            );
            events.push(Event::Request {
              stream: id,
              head: Request::from_parts(head, ()),
              end,
            });
          }
          Poll::Ready(None) => {
            self.state = State::Closed;
            break;
          }
          Poll::Ready(Some(Err(error))) => {
            self.state = State::Closed;
            return Err(io::Error::other(error));
          }
          Poll::Pending => break,
        }
      }
    }
    if let State::Connected(connection) = &mut self.state {
      match connection.poll_closed(cx) {
        Poll::Ready(Ok(())) => self.state = State::Closed,
        Poll::Ready(Err(error)) => {
          self.state = State::Closed;
          return Err(io::Error::other(error));
        }
        Poll::Pending => {}
      }
    }
    for (&id, stream) in &mut self.streams {
      if events.len() >= limit {
        cx.waker().wake_by_ref();
        break;
      }
      if stream.reset {
        continue;
      }
      let reset = match &mut stream.send {
        Some(send) => send.poll_reset(cx),
        None => stream.respond.poll_reset(cx),
      };
      if let Poll::Ready(reason) = reset {
        stream.reset = true;
        events.push(Event::Reset {
          stream: id,
          reason: reason.unwrap_or(Reason::INTERNAL_ERROR),
        });
        continue;
      }
      if stream.awaiting_capacity
        && let Some(send) = &mut stream.send
        && let Poll::Ready(Some(Ok(capacity))) = send.poll_capacity(cx)
        && capacity > 0
      {
        stream.awaiting_capacity = false;
        events.push(Event::Writable { stream: id });
      }
      if events.len() >= limit {
        cx.waker().wake_by_ref();
        break;
      }
      if !stream.received_end {
        match stream.recv.poll_data(cx) {
          Poll::Ready(Some(Ok(bytes))) => {
            stream.unacked += bytes.len();
            events.push(Event::Data { stream: id, bytes });
          }
          Poll::Ready(Some(Err(error))) => {
            stream.reset = true;
            events.push(Event::Reset {
              stream: id,
              reason: error.reason().unwrap_or(Reason::INTERNAL_ERROR),
            });
          }
          Poll::Ready(None) => {
            // Poll trailers so END_STREAM after a trailer block is consumed too.
            match stream.recv.poll_trailers(cx) {
              Poll::Ready(Ok(_)) => {
                stream.received_end = true;
                events.push(Event::End { stream: id });
              }
              Poll::Ready(Err(error)) => {
                stream.reset = true;
                events.push(Event::Reset {
                  stream: id,
                  reason: error.reason().unwrap_or(Reason::INTERNAL_ERROR),
                });
              }
              Poll::Pending => {}
            }
          }
          Poll::Pending => {}
        }
      }
    }
    if !events.is_empty() {
      cx.waker().wake_by_ref();
    }
    Ok(events)
  }

  pub(crate) fn response(&mut self, id: u32, head: Response<()>, end: bool) -> io::Result<()> {
    let stream = self.streams.get_mut(&id).ok_or(io::ErrorKind::NotFound)?;
    if stream.reset || stream.send.is_some() {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    if head.status().is_informational() {
      return stream.respond.send_informational(head).map_err(io::Error::other);
    }
    stream.send = Some(stream.respond.send_response(head, end).map_err(io::Error::other)?);
    stream.sent_end = end;
    Ok(())
  }

  /// Queue at most the currently granted flow-control capacity, returning the accepted prefix.
  /// `end` applies only when every supplied byte is accepted. Zero means wait for Writable.
  pub(crate) fn data(&mut self, id: u32, bytes: Bytes, end: bool) -> io::Result<usize> {
    let stream = self.streams.get_mut(&id).ok_or(io::ErrorKind::NotFound)?;
    if stream.reset || stream.sent_end {
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    let send = stream.send.as_mut().ok_or(io::ErrorKind::InvalidInput)?;
    send.reserve_capacity(bytes.len().min(WINDOW));
    let accepted = bytes.len().min(send.capacity());
    if accepted == 0 && !bytes.is_empty() {
      stream.awaiting_capacity = true;
      return Ok(0);
    }
    let final_part = end && accepted == bytes.len();
    send
      .send_data(bytes.slice(..accepted), final_part)
      .map_err(io::Error::other)?;
    stream.sent_end = final_part;
    stream.awaiting_capacity = accepted < bytes.len();
    Ok(accepted)
  }

  pub(crate) fn ack(&mut self, id: u32, bytes: usize) -> io::Result<()> {
    let stream = self.streams.get_mut(&id).ok_or(io::ErrorKind::NotFound)?;
    if bytes > stream.unacked {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    stream
      .recv
      .flow_control()
      .release_capacity(bytes)
      .map_err(io::Error::other)?;
    stream.unacked -= bytes;
    Ok(())
  }

  pub(crate) fn reset(&mut self, id: u32, reason: Reason) {
    if let Some(stream) = self.streams.get_mut(&id) {
      stream.respond.send_reset(reason);
      stream.reset = true;
    }
  }

  /// Retire application state; incomplete exchanges are explicitly cancelled.
  pub(crate) fn release(&mut self, id: u32) {
    if let Some(mut stream) = self.streams.remove(&id)
      && (!stream.received_end || !stream.sent_end)
    {
      stream.respond.send_reset(Reason::CANCEL);
    }
  }

  /// Input is closed and accepted streams have drained; no peer PONG can arrive now.
  pub(crate) fn finish_shutdown(&mut self) {
    if let State::Connected(connection) = &mut self.state {
      connection.abrupt_shutdown(Reason::NO_ERROR);
    }
  }

  pub(crate) fn drain(&mut self) {
    self.draining = true;
    if let State::Connected(connection) = &mut self.state {
      connection.graceful_shutdown();
    }
  }
}

#[cfg(test)]
pub(crate) mod tests {
  use super::*;

  pub(crate) struct Pair {
    pub(crate) server: Connection,
    client_io: MemoryIo,
    client: Pin<Box<h2::client::Connection<MemoryIo, Bytes>>>,
    pub(crate) sender: h2::client::SendRequest<Bytes>,
    closed: bool,
    pub(crate) marks: Vec<FrameMark>,
  }

  impl Pair {
    pub(crate) fn new(window: u32) -> Self {
      let client_io = MemoryIo::default();
      let mut handshake = Box::pin(
        h2::client::Builder::new()
          .initial_window_size(window)
          .handshake(client_io.clone()),
      );
      let mut cx = Context::from_waker(Waker::noop());
      let Poll::Ready(Ok((sender, client))) = handshake.as_mut().poll(&mut cx) else {
        panic!("in-memory client handshake should write its preface immediately");
      };
      let mut pair = Self {
        server: Connection::new(),
        client_io,
        client: Box::pin(client),
        sender,
        closed: false,
        marks: Vec::new(),
      };
      for _ in 0..8 {
        assert!(pair.tick().is_empty());
      }
      pair
    }

    pub(crate) fn tick(&mut self) -> Vec<Event> {
      let mut cx = Context::from_waker(Waker::noop());
      if !self.closed
        && let Poll::Ready(result) = self.client.as_mut().poll(&mut cx)
      {
        result.unwrap();
        self.closed = true;
      }
      while let Some(bytes) = self.client_io.output() {
        self.server.feed(bytes).unwrap();
      }
      let events = self.server.poll(&mut cx, 256).unwrap();
      self.marks.extend(self.server.take_frames());
      while let Some(bytes) = self.server.take_output() {
        self.client_io.feed(bytes).unwrap();
      }
      events
    }

    pub(crate) fn requests(&mut self, count: usize) -> Vec<(u32, String, bool)> {
      let mut found = Vec::new();
      for _ in 0..64 {
        for event in self.tick() {
          if let Event::Request { stream, head, end } = event {
            found.push((stream, head.uri().path().to_owned(), end));
          }
        }
        if found.len() >= count {
          return found;
        }
      }
      panic!("missing requests");
    }
  }

  fn request(path: &str) -> Request<()> {
    Request::builder()
      .uri(format!("https://localhost{path}"))
      .body(())
      .unwrap()
  }

  #[test]
  fn independent_stream_responses_need_no_tokio_runtime() {
    let mut pair = Pair::new(65535);
    let (mut first, _) = pair.sender.send_request(request("/first"), true).unwrap();
    let (mut second, _) = pair.sender.send_request(request("/second"), true).unwrap();
    let requests = pair.requests(2);
    assert_eq!(requests[0].1, "/first");
    assert!(requests.iter().all(|(_, _, end)| *end));
    pair
      .server
      .response(requests[1].0, Response::builder().status(201).body(()).unwrap(), true)
      .unwrap();
    for _ in 0..8 {
      pair.tick();
    }
    let mut cx = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut first).poll(&mut cx).is_pending());
    let Poll::Ready(Ok(response)) = Pin::new(&mut second).poll(&mut cx) else {
      panic!("second response missing")
    };
    assert_eq!(response.status(), 201);
    pair.server.response(requests[0].0, Response::new(()), true).unwrap();
    for _ in 0..8 {
      pair.tick();
    }
    assert!(matches!(Pin::new(&mut first).poll(&mut cx), Poll::Ready(Ok(_))));
    for (id, _, _) in requests {
      pair.server.release(id);
    }
    assert!(pair.server.streams.is_empty());
  }

  #[test]
  fn request_flow_control_stalls_until_application_acknowledges() {
    let mut pair = Pair::new(65535);
    let (_response, mut send) = pair.sender.send_request(request("/body"), false).unwrap();
    let id = pair.requests(1)[0].0;
    send.send_data(Bytes::from(vec![42; WINDOW * 2]), true).unwrap();
    let mut received = 0;
    let mut ended = false;
    for _ in 0..80 {
      for event in pair.tick() {
        match event {
          Event::Data { stream, bytes } => {
            assert_eq!(stream, id);
            received += bytes.len();
          }
          Event::End { .. } => ended = true,
          _ => {}
        }
      }
    }
    assert_eq!(received, WINDOW);
    assert!(!ended);
    assert!(pair.server.ack(id, received + 1).is_err());
    pair.server.ack(id, received).unwrap();
    for _ in 0..80 {
      for event in pair.tick() {
        match event {
          Event::Data { bytes, .. } => received += bytes.len(),
          Event::End { stream } => {
            assert_eq!(stream, id);
            ended = true;
          }
          _ => {}
        }
      }
    }
    assert_eq!(received, WINDOW * 2);
    assert!(ended);
  }

  #[test]
  fn response_flow_control_stalls_until_peer_releases_capacity() {
    let mut pair = Pair::new(1024);
    let (mut response, _) = pair.sender.send_request(request("/"), true).unwrap();
    let id = pair.requests(1)[0].0;
    pair.server.response(id, Response::new(()), false).unwrap();
    let payload = Bytes::from(vec![7; 2048]);
    assert_eq!(pair.server.data(id, payload.clone(), true).unwrap(), 1024);
    for _ in 0..8 {
      pair.tick();
    }
    assert_eq!(pair.server.data(id, payload.slice(1024..), true).unwrap(), 0);
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Ok(response)) = Pin::new(&mut response).poll(&mut cx) else {
      panic!("response missing")
    };
    let mut recv = response.into_body();
    let Poll::Ready(Some(Ok(data))) = recv.poll_data(&mut cx) else {
      panic!("response DATA missing")
    };
    assert_eq!(data.len(), 1024);
    recv.flow_control().release_capacity(data.len()).unwrap();
    let mut writable = false;
    for _ in 0..8 {
      writable |= pair
        .tick()
        .into_iter()
        .any(|event| matches!(event, Event::Writable { stream } if stream == id));
    }
    assert!(writable);
    assert_eq!(pair.server.data(id, payload.slice(1024..), true).unwrap(), 1024);
    for _ in 0..8 {
      pair.tick();
    }
    assert!(matches!(recv.poll_data(&mut cx), Poll::Ready(Some(Ok(bytes))) if bytes.len() == 1024));
    assert!(matches!(recv.poll_data(&mut cx), Poll::Ready(None)));
    let frames: Vec<_> = pair
      .marks
      .iter()
      .filter(|mark| mark.stream == id && mark.kind == DATA)
      .collect();
    assert_eq!(frames.iter().map(|mark| mark.payload_bytes).sum::<usize>(), 2048);
    assert!(frames.last().unwrap().flags & END_STREAM != 0);
  }

  #[test]
  fn peer_reset_is_stream_local_and_goaway_finishes_existing_response() {
    let mut pair = Pair::new(65535);
    let (_cancelled, mut cancelled) = pair.sender.send_request(request("/cancel"), false).unwrap();
    let (mut response, _) = pair.sender.send_request(request("/keep"), true).unwrap();
    let requests = pair.requests(2);
    cancelled.send_reset(Reason::CANCEL);
    let mut reset = false;
    for _ in 0..8 {
      reset |= pair.tick().into_iter().any(
        |event| matches!(event, Event::Reset { stream, reason } if stream == requests[0].0 && reason == Reason::CANCEL),
      );
    }
    assert!(reset);
    pair.server.release(requests[0].0);
    pair.server.drain();
    pair.server.response(requests[1].0, Response::new(()), true).unwrap();
    for _ in 0..16 {
      pair.tick();
    }
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(Pin::new(&mut response).poll(&mut cx), Poll::Ready(Ok(_))));
    assert!(pair.sender.send_request(request("/late"), true).is_err());
  }

  #[test]
  fn memory_adapter_and_protocol_input_are_bounded() {
    let mut conn = Connection::new();
    conn.feed(Bytes::from(vec![0; WINDOW])).unwrap();
    assert_eq!(conn.input_capacity(), 0);
    assert_eq!(
      conn.feed(Bytes::from_static(b"x")).unwrap_err().kind(),
      io::ErrorKind::WouldBlock
    );
    let mut cx = Context::from_waker(Waker::noop());
    assert!(conn.poll(&mut cx, 64).is_err());
    assert!(conn.is_closed());
    let mut wire = MemoryIo::default();
    let bytes = vec![1; WINDOW + 1];
    assert!(matches!(
      Pin::new(&mut wire).poll_write(&mut cx, &bytes),
      Poll::Ready(Ok(WINDOW))
    ));
    assert!(Pin::new(&mut wire).poll_write(&mut cx, b"x").is_pending());
    assert_eq!(wire.output().unwrap().len(), WINDOW);
    assert!(matches!(
      Pin::new(&mut wire).poll_write(&mut cx, b"x"),
      Poll::Ready(Ok(1))
    ));
  }

  #[test]
  fn stream_admission_resumes_after_a_completed_exchange_is_released() {
    let mut pair = Pair::new(65535);
    let mut responses = Vec::new();
    for _ in 0..MAX_STREAMS {
      responses.push(pair.sender.send_request(request("/"), true).unwrap().0);
    }
    let requests = pair.requests(MAX_STREAMS);
    assert_eq!(pair.server.streams.len(), MAX_STREAMS);
    responses.push(pair.sender.send_request(request("/later"), true).unwrap().0);
    for _ in 0..8 {
      assert!(pair.tick().is_empty());
    }
    pair.server.response(requests[0].0, Response::new(()), true).unwrap();
    pair.server.release(requests[0].0);
    let later = pair.requests(1);
    assert_eq!(later[0].1, "/later");
    assert_eq!(pair.server.streams.len(), MAX_STREAMS);
  }

  #[test]
  fn oversized_header_list_is_rejected_without_dispatch() {
    let mut pair = Pair::new(65535);
    let oversized = Request::builder()
      .uri("https://localhost/")
      .header("x-large", "x".repeat(HEAD_LIMIT as usize + 1))
      .body(())
      .unwrap();
    let (mut response, _) = pair.sender.send_request(oversized, true).unwrap();
    for _ in 0..16 {
      assert!(
        !pair
          .tick()
          .into_iter()
          .any(|event| matches!(event, Event::Request { .. }))
      );
    }
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(Pin::new(&mut response).poll(&mut cx), Poll::Ready(Ok(response)) if response.status() == 431));
  }

  #[test]
  fn local_reset_reaches_peer_and_eof_finishes_connection() {
    let mut pair = Pair::new(65535);
    let (mut response, _) = pair.sender.send_request(request("/"), true).unwrap();
    let id = pair.requests(1)[0].0;
    pair.server.reset(id, Reason::CANCEL);
    for _ in 0..8 {
      pair.tick();
    }
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Err(error)) = Pin::new(&mut response).poll(&mut cx) else {
      panic!("reset missing")
    };
    assert_eq!(error.reason(), Some(Reason::CANCEL));
    pair.server.release(id);
    pair.server.eof();
    for _ in 0..8 {
      if pair.server.poll(&mut cx, 64).is_err() {
        break;
      }
    }
    assert!(pair.server.is_closed());
  }

  #[test]
  fn frame_marks_follow_fragmented_headers_continuations_and_empty_final_data() {
    let wire = [
      0,
      0,
      2,
      HEADERS,
      0,
      0,
      0,
      0,
      3,
      0xaa,
      0xbb,
      0,
      0,
      1,
      CONTINUATION,
      END_HEADERS,
      0,
      0,
      0,
      3,
      0xcc,
      0,
      0,
      3,
      DATA,
      0,
      0,
      0,
      0,
      3,
      1,
      2,
      3,
      0,
      0,
      0,
      DATA,
      END_STREAM,
      0,
      0,
      0,
      3,
    ];
    for split in 1..=wire.len() {
      let mut tracker = FrameTracker::default();
      for chunk in wire.chunks(split) {
        tracker.record(chunk).unwrap();
      }
      let frames: Vec<_> = tracker.marks.into_iter().collect();
      assert_eq!(frames.len(), 4);
      assert!(frames.iter().all(|mark| mark.stream == 3));
      assert_eq!(
        frames.iter().map(|mark| mark.wire_end).collect::<Vec<_>>(),
        [11, 21, 33, 42]
      );
      assert_eq!(frames[0].kind, HEADERS);
      assert_eq!(frames[1].kind, CONTINUATION);
      assert_eq!(frames[1].flags, END_HEADERS);
      assert_eq!(frames[2].payload_bytes, 3);
      assert_eq!(frames[3].payload_bytes, 0);
      assert_eq!(frames[3].flags, END_STREAM);
    }
  }

  #[test]
  fn padded_data_frames_are_rejected_by_the_tracker() {
    let mut tracker = FrameTracker::default();
    let error = tracker.record(&[0, 0, 2, DATA, 8, 0, 0, 0, 1, 0, 0]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
  }

  #[test]
  fn memory_io_handles_empty_transfers_eof_and_wakeups() {
    let mut cx = Context::from_waker(Waker::noop());
    let mut wire = MemoryIo::default();
    let mut none: [u8; 0] = [];
    let mut empty = ReadBuf::new(&mut none);
    assert!(matches!(
      Pin::new(&mut wire).poll_read(&mut cx, &mut empty),
      Poll::Ready(Ok(()))
    ));
    assert!(matches!(
      Pin::new(&mut wire).poll_write(&mut cx, b""),
      Poll::Ready(Ok(0))
    ));
    wire.feed(Bytes::new()).unwrap();
    assert_eq!(wire.0.borrow().input_bytes, 0);
    let mut storage = [0u8; 4];
    let mut buf = ReadBuf::new(&mut storage);
    assert!(Pin::new(&mut wire).poll_read(&mut cx, &mut buf).is_pending());
    assert!(wire.0.borrow().reader.is_some());
    wire.feed(Bytes::from_static(b"ab")).unwrap();
    assert!(wire.0.borrow().reader.is_none(), "feeding wakes the parked reader");
    assert!(matches!(
      Pin::new(&mut wire).poll_read(&mut cx, &mut buf),
      Poll::Ready(Ok(()))
    ));
    assert_eq!(buf.filled(), b"ab");
    wire.0.borrow_mut().eof = true;
    let mut rest = [0u8; 4];
    let mut buf = ReadBuf::new(&mut rest);
    assert!(matches!(
      Pin::new(&mut wire).poll_read(&mut cx, &mut buf),
      Poll::Ready(Ok(()))
    ));
    assert!(buf.filled().is_empty(), "EOF reads complete without bytes");
    assert_eq!(
      wire.feed(Bytes::from_static(b"x")).unwrap_err().kind(),
      io::ErrorKind::BrokenPipe
    );
  }

  #[test]
  fn draining_frames_wakes_a_writer_parked_on_a_full_window() {
    let mut connection = Connection::new();
    let mut io = connection.io.clone();
    let mut cx = Context::from_waker(Waker::noop());
    let full = vec![0u8; WINDOW];
    assert!(matches!(
      Pin::new(&mut io).poll_write(&mut cx, &full),
      Poll::Ready(Ok(WINDOW))
    ));
    assert!(Pin::new(&mut io).poll_write(&mut cx, b"x").is_pending());
    assert!(io.0.borrow().writer.is_some());
    assert!(!connection.take_frames().is_empty());
    assert!(io.0.borrow().writer.is_none());
  }

  #[test]
  fn polling_respects_zero_limits_and_a_pending_handshake() {
    let mut connection = Connection::new();
    let mut cx = Context::from_waker(Waker::noop());
    assert!(connection.poll(&mut cx, 0).unwrap().is_empty());
    assert!(connection.poll(&mut cx, 8).unwrap().is_empty());
    assert!(!connection.is_closed());
    assert!(
      connection.take_output().is_some(),
      "the server preface is written eagerly"
    );
  }

  #[test]
  fn event_limits_defer_remaining_streams_to_the_next_poll() {
    let mut pair = Pair::new(65535);
    let _first = pair.sender.send_request(request("/a"), true).unwrap();
    let _second = pair.sender.send_request(request("/b"), true).unwrap();
    let mut cx = Context::from_waker(Waker::noop());
    assert!(pair.client.as_mut().poll(&mut cx).is_pending());
    while let Some(bytes) = pair.client_io.output() {
      pair.server.feed(bytes).unwrap();
    }
    let mut paths = Vec::new();
    for _ in 0..8 {
      let events = pair.server.poll(&mut cx, 1).unwrap();
      assert!(events.len() <= 1);
      for event in events {
        if let Event::Request { head, .. } = event {
          paths.push(head.uri().path().to_owned());
        }
      }
    }
    assert_eq!(paths, ["/a", "/b"]);
  }

  #[test]
  fn response_and_data_calls_validate_stream_state() {
    let mut pair = Pair::new(65535);
    let (mut response, _) = pair.sender.send_request(request("/"), true).unwrap();
    let id = pair.requests(1)[0].0;
    assert_eq!(
      pair
        .server
        .response(id + 2, Response::new(()), true)
        .unwrap_err()
        .kind(),
      io::ErrorKind::NotFound
    );
    assert_eq!(
      pair.server.data(id, Bytes::from_static(b"x"), true).unwrap_err().kind(),
      io::ErrorKind::InvalidInput,
      "DATA needs a response head first"
    );
    assert_eq!(pair.server.ack(id + 2, 0).unwrap_err().kind(), io::ErrorKind::NotFound);
    pair
      .server
      .response(id, Response::builder().status(103).body(()).unwrap(), false)
      .unwrap();
    pair.server.response(id, Response::new(()), false).unwrap();
    assert_eq!(
      pair.server.response(id, Response::new(()), true).unwrap_err().kind(),
      io::ErrorKind::InvalidInput
    );
    assert_eq!(pair.server.data(id, Bytes::from_static(b"ok"), true).unwrap(), 2);
    assert_eq!(
      pair
        .server
        .data(id, Bytes::from_static(b"late"), false)
        .unwrap_err()
        .kind(),
      io::ErrorKind::BrokenPipe
    );
    for _ in 0..8 {
      pair.tick();
    }
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Ok(reply)) = Pin::new(&mut response).poll(&mut cx) else {
      panic!("final response missing")
    };
    assert_eq!(reply.status(), 200);
    let mut body = reply.into_body();
    assert!(matches!(body.poll_data(&mut cx), Poll::Ready(Some(Ok(bytes))) if bytes.as_ref() == b"ok"));
    pair.server.reset(id + 2, Reason::CANCEL);
    pair.server.release(id);
    assert!(pair.server.streams.is_empty());
  }

  #[test]
  fn body_longer_than_its_content_length_resets_the_stream() {
    let mut pair = Pair::new(65535);
    let head = Request::builder()
      .method("POST")
      .uri("https://localhost/length")
      .header("content-length", "1")
      .body(())
      .unwrap();
    let (_response, mut send) = pair.sender.send_request(head, false).unwrap();
    let id = pair.requests(1)[0].0;
    send.send_data(Bytes::from_static(b"hello"), true).unwrap();
    let mut reset = false;
    for _ in 0..16 {
      reset |= pair
        .tick()
        .into_iter()
        .any(|event| matches!(event, Event::Reset { stream, .. } if stream == id));
    }
    assert!(reset);
    assert!(pair.server.response(id, Response::new(()), true).is_err());
  }

  #[test]
  fn draining_before_the_handshake_completes_closes_after_connect() {
    let client_io = MemoryIo::default();
    let mut handshake = Box::pin(h2::client::Builder::new().handshake::<_, Bytes>(client_io.clone()));
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Ok((sender, client))) = handshake.as_mut().poll(&mut cx) else {
      panic!("in-memory client handshake should write its preface immediately");
    };
    let mut server = Connection::new();
    server.drain();
    let mut pair = Pair {
      server,
      client_io,
      client: Box::pin(client),
      sender,
      closed: false,
      marks: Vec::new(),
    };
    for _ in 0..16 {
      assert!(
        !pair
          .tick()
          .into_iter()
          .any(|event| matches!(event, Event::Request { .. }))
      );
    }
    assert!(pair.server.is_closed());
    assert!(pair.server.poll(&mut cx, 8).unwrap().is_empty());
  }

  #[test]
  fn connection_errors_after_the_handshake_close_the_connection() {
    let mut pair = Pair::new(65535);
    // SETTINGS with a length that is not a multiple of six is a connection FRAME_SIZE_ERROR.
    pair
      .server
      .feed(Bytes::from_static(&[0, 0, 1, 4, 0, 0, 0, 0, 0, 0]))
      .unwrap();
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..8 {
      if pair.server.poll(&mut cx, 8).is_err() {
        break;
      }
    }
    assert!(pair.server.is_closed());
  }

  #[test]
  fn bounded_output_slices_preserve_frame_completion_offsets() {
    let mut connection = Connection::new();
    let wire = [0, 0, 3, DATA, END_STREAM, 0, 0, 0, 3, 1, 2, 3];
    let mut io = connection.io.clone();
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
      Pin::new(&mut io).poll_write(&mut cx, &wire),
      Poll::Ready(Ok(12))
    ));
    assert!(connection.take_output_limit(0).is_none());
    assert_eq!(connection.take_output_limit(5).unwrap().as_ref(), &wire[..5]);
    let marks = connection.take_frames();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].wire_end, 12);
    assert_eq!(connection.take_output_limit(5).unwrap().as_ref(), &wire[5..10]);
    assert_eq!(connection.take_output_limit(5).unwrap().as_ref(), &wire[10..]);
    assert!(connection.take_output().is_none());
  }
}
