/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Native HTTP/2 exchanges and driver-owned stream lifetimes.

use super::*;
use crate::http::{HeaderSpan, MAX_HEAD_BYTES, MAX_HEADERS, ParseError};

use crate::http::h2::{CONTINUATION, Connection, END_HEADERS, Event, FrameMark, HEADERS};
use bytes::Bytes;
use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Wake, Waker};

#[derive(Default)]
struct WakeFlag(AtomicBool);
impl Wake for WakeFlag {
  fn wake(self: Arc<Self>) {
    self.0.store(true, Ordering::Release);
  }
  fn wake_by_ref(self: &Arc<Self>) {
    self.0.store(true, Ordering::Release);
  }
}

#[path = "http_h2_response.rs"]
mod response;
use response::Response;

pub(super) struct H2Socket {
  connection: Connection,
  streams: IntMap<u32, Stream>,
  wire: VecDeque<FrozenBuffer>,
  wire_bytes: usize,
  wake: Arc<WakeFlag>,
  marks: VecDeque<FrameMark>,
  sent: u64,
  driving: bool,
  draining: bool,
  peer_closed: bool,
}

struct Stream {
  exchange: u64,
  response: Option<Response>,
  body_ended: bool,
  discarded: bool,
  freed: bool,
  reset: bool,
  interim: usize,
}

impl H2Socket {
  pub(super) fn needs_poll(&self) -> bool {
    self.wake.0.load(Ordering::Acquire)
  }

  pub(super) fn input_capacity(&self) -> usize {
    self.connection.input_capacity()
  }

  pub(super) fn new() -> Self {
    Self {
      connection: Connection::new(),
      streams: IntMap::default(),
      wire: VecDeque::new(),
      wire_bytes: 0,
      wake: Arc::default(),
      marks: VecDeque::new(),
      sent: 0,
      driving: false,
      draining: false,
      peer_closed: false,
    }
  }
}

pub(super) fn feed(state: &mut DriverState, socket: u64, storage: FrozenBuffer, events: &mut VecDeque<NativeEvent>) {
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let h2 = http.h2.get_or_insert_with(H2Socket::new);
  if h2.connection.feed(Bytes::from_owner(storage)).is_err() {
    close_socket(state, socket, events);
    return;
  }
  drive(state, socket, events);
}

pub(super) fn drive(state: &mut DriverState, socket: u64, events: &mut VecDeque<NativeEvent>) {
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let Some(h2) = &mut http.h2 else {
    return;
  };
  if h2.driving {
    return;
  }
  h2.driving = true;
  if http.draining && !h2.draining {
    h2.draining = true;
    h2.connection.drain();
  }
  let result = (|| -> io::Result<()> {
    for _ in 0..4 {
      if h2.peer_closed && h2.streams.is_empty() {
        h2.connection.finish_shutdown();
      }
      for (&id, stream) in &mut h2.streams {
        if stream.reset {
          continue;
        }
        if let Some(response) = &mut stream.response {
          if response.progress(&mut h2.connection, id).is_err() {
            h2.connection.reset(id, ::h2::Reason::INTERNAL_ERROR);
            stream.reset = true;
            let failures = response.cancel();
            if !stream.freed {
              if !stream.body_ended {
                events.push_back(event(EVENT_BODY_END, 0, socket, stream.exchange, aborted()));
              }
              stream.body_ended = true;
              for failure in failures {
                events.push_back(event(EVENT_PART_SENT, 0, socket, stream.exchange, failure));
              }
              events.push_back(event(EVENT_RESET, 0, socket, stream.exchange, aborted()));
            }
            continue;
          }
          for sent in response.take_events() {
            if !stream.freed {
              events.push_back(event(EVENT_PART_SENT, 0, socket, stream.exchange, sent));
            }
          }
        }
      }
      h2.wake.0.store(false, Ordering::Release);
      let waker = Waker::from(h2.wake.clone());
      let mut cx = Context::from_waker(&waker);
      for message in h2.connection.poll(&mut cx, MAX_EXCHANGES)? {
        match message {
          Event::Request { stream, head, end } => {
            if http.draining {
              h2.connection.reset(stream, ::h2::Reason::REFUSED_STREAM);
              h2.connection.release(stream);
              continue;
            }
            let parsed = match exchange(head, end, &http.budget) {
              Ok(parsed) => parsed,
              Err(error) => {
                let response = Response::new(
                  &mut h2.connection,
                  stream,
                  error.status(),
                  &[],
                  &[],
                  0,
                  false,
                  false,
                  &http.budget,
                )?;
                h2.streams.insert(
                  stream,
                  Stream {
                    exchange: 0,
                    response: Some(response),
                    body_ended: end,
                    discarded: true,
                    freed: true,
                    reset: false,
                    interim: 0,
                  },
                );
                continue;
              }
            };
            let expect_continue = parsed.expect_continue;
            let id = state.http.exchanges.insert(socket, parsed, http.budget.clone(), stream);
            http.outstanding += 1;
            h2.streams.insert(
              stream,
              Stream {
                exchange: id,
                response: None,
                body_ended: end,
                discarded: false,
                freed: false,
                reset: false,
                interim: usize::from(expect_continue),
              },
            );
            if expect_continue {
              h2.connection
                .response(stream, ::http::Response::builder().status(100).body(()).unwrap(), false)?;
            }
            events.push_back(event(EVENT_REQUEST, 0, socket, id, 0));
          }
          Event::Data { stream, bytes } => {
            let Some(request) = h2.streams.get(&stream) else {
              continue;
            };
            if request.discarded || request.freed {
              h2.connection.ack(stream, bytes.len())?;
              continue;
            }
            if bytes.is_empty() {
              continue;
            }
            let mut native = Buffer::new(bytes.len(), http.budget.clone())?;
            native.write(0, &bytes)?;
            let bytes = native.freeze();
            let length = bytes.as_ref().len();
            let storage = Box::new(SegmentStorage {
              layout: SegmentLayout {
                data: bytes.as_ref().as_ptr() as u64,
                len: length as u64,
              },
              bytes,
            });
            let id = &*storage as *const SegmentStorage as u64;
            state.http.segments.insert(
              id,
              BodySegment {
                storage,
                socket,
                charge: Some(ReceiveCharge::new(length, &http.pinned)),
                h2: Some((stream, length)),
              },
            );
            events.push_back(event(EVENT_BODY, id, socket, request.exchange, length as i64));
          }
          Event::End { stream } => {
            if let Some(request) = h2.streams.get_mut(&stream) {
              if !request.body_ended && !request.freed {
                events.push_back(event(EVENT_BODY_END, 0, socket, request.exchange, 0));
              }
              request.body_ended = true;
            }
          }
          Event::Reset {
            stream,
            reason: _reason,
          } => {
            if let Some(request) = h2.streams.get_mut(&stream) {
              request.reset = true;
              if !request.freed {
                if !request.body_ended {
                  events.push_back(event(EVENT_BODY_END, 0, socket, request.exchange, aborted()));
                }
                if let Some(response) = &mut request.response {
                  for result in response.cancel() {
                    events.push_back(event(EVENT_PART_SENT, 0, socket, request.exchange, result));
                  }
                }
              }
              request.body_ended = true;
              if !request.freed {
                events.push_back(event(EVENT_RESET, 0, socket, request.exchange, aborted()));
              }
            }
          }
          Event::Writable { stream: _stream } => {}
        }
      }
      while h2.wire.len() < SEND_PARTS && h2.wire_bytes < crate::http::h2::WINDOW {
        let Some(bytes) = h2.connection.take_output_limit(crate::http::h2::WINDOW - h2.wire_bytes) else {
          break;
        };
        h2.wire_bytes += bytes.len();
        let mut native = Buffer::new(bytes.len().max(1), http.budget.clone())?;
        native.write(0, &bytes)?;
        h2.wire.push_back(native.freeze());
      }
      h2.marks.extend(h2.connection.take_frames());
    }
    Ok(())
  })();
  h2.driving = false;
  if result.is_err() {
    close_socket(state, socket, events);
    return;
  }
  retire(http, &mut state.http.exchanges);
  if !http.sending && !http.h2.as_ref().unwrap().wire.is_empty() {
    let h2 = http.h2.as_mut().unwrap();
    let batch = h2.wire.drain(..).collect();
    h2.wire_bytes = 0;
    if let Some(tls) = &mut http.tls {
      tls.lane.enqueue(batch);
      http.sending = true;
    }
  }
  let should_close = {
    let h2 = http.h2.as_ref().unwrap();
    h2.connection.is_closed() && h2.wire.is_empty() && !http.sending
  };
  if should_close && let Some(tls) = &mut http.tls {
    tls.lane.close();
  }
}

fn retire(http: &mut HttpSocket, exchanges: &mut ExchangeStore) {
  let Some(h2) = &mut http.h2 else {
    return;
  };
  let retired: Vec<u32> = h2
    .streams
    .iter()
    .filter_map(|(&id, stream)| {
      (stream.freed && (stream.reset || stream.response.as_ref().is_some_and(Response::wire_complete))).then_some(id)
    })
    .collect();
  for id in retired {
    let stream = h2.streams.remove(&id).unwrap();
    h2.connection.release(id);
    if stream.exchange != 0 {
      http.retired.remove(&stream.exchange);
      exchanges.recycle(stream.exchange);
      http.outstanding -= 1;
    }
  }
}

pub(super) fn sent(state: &mut DriverState, socket: u64, length: usize, events: &mut VecDeque<NativeEvent>) {
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let h2 = http.h2.as_mut().unwrap();
  h2.sent += length as u64;
  while h2.marks.front().is_some_and(|mark| mark.wire_end <= h2.sent) {
    let mark = h2.marks.pop_front().unwrap();
    let Some(stream) = h2.streams.get_mut(&mark.stream) else {
      continue;
    };
    if stream.interim != 0 && matches!(mark.kind, HEADERS | CONTINUATION) {
      if mark.flags & END_HEADERS != 0 {
        stream.interim -= 1;
      }
      continue;
    }
    if let Some(response) = &mut stream.response {
      response.acknowledge(&mark);
      for result in response.take_events() {
        if !stream.freed {
          events.push_back(event(EVENT_PART_SENT, 0, socket, stream.exchange, result));
        }
      }
    }
  }
  http.sending = false;
  retire(http, &mut state.http.exchanges);
  drive(state, socket, events);
}

pub(super) fn ack(state: &mut DriverState, socket: u64, stream: u32, length: usize) {
  if let Some(h2) = state.http.sockets.get_mut(&socket).and_then(|http| http.h2.as_mut()) {
    let _ = h2.connection.ack(stream, length);
  }
}

fn exchange(head: ::http::Request<()>, end: bool, budget: &Budget) -> Result<Exchange, ParseError> {
  let method = head.method().as_str().as_bytes();
  let path = head.uri().path_and_query().map_or("/", |path| path.as_str()).as_bytes();
  let mut bytes = Vec::new();
  bytes.extend_from_slice(method);
  bytes.push(b' ');
  let path_start = bytes.len() as u32;
  bytes.extend_from_slice(path);
  let path_end = bytes.len() as u32;
  bytes.extend_from_slice(b" HTTP/2.0\r\n");
  let mut headers = Vec::new();
  let mut expect_continue = false;
  let mut content_length = None;
  let mut content_length_seen = false;
  let mut host_index = None;
  let mut append = |name: &[u8], value: &[u8]| -> Result<(), ParseError> {
    if headers.len() == MAX_HEADERS || bytes.len() + name.len() + value.len() + 6 > MAX_HEAD_BYTES {
      return Err(ParseError::HeadTooLarge);
    }
    if name.eq_ignore_ascii_case(b"host") {
      host_index.get_or_insert(headers.len() as u32);
    } else if name.eq_ignore_ascii_case(b"content-length") && !content_length_seen {
      content_length_seen = true;
      content_length = std::str::from_utf8(value).ok().and_then(|value| value.parse().ok());
    }
    let name_start = bytes.len() as u32;
    bytes.extend_from_slice(name);
    let name_end = bytes.len() as u32;
    bytes.extend_from_slice(b": ");
    let value_start = bytes.len() as u32;
    bytes.extend_from_slice(value);
    let value_end = bytes.len() as u32;
    bytes.extend_from_slice(b"\r\n");
    headers.push(HeaderSpan {
      name: name_start..name_end,
      value: value_start..value_end,
    });
    Ok(())
  };
  if !head.headers().contains_key(::http::header::HOST)
    && let Some(authority) = head.uri().authority()
  {
    append(b"host", authority.as_str().as_bytes())?;
  }
  for (name, value) in head.headers() {
    if name == ::http::header::EXPECT {
      for expectation in value.as_bytes().split(|b| *b == b',') {
        let expectation = expectation.trim_ascii();
        if expectation.is_empty() {
          continue;
        }
        if !expectation.eq_ignore_ascii_case(b"100-continue") {
          return Err(ParseError::UnsupportedExpectation);
        }
        expect_continue = !end;
      }
    }
    append(name.as_str().as_bytes(), value.as_bytes())?;
  }
  bytes.extend_from_slice(b"\r\n");
  if bytes.len() > MAX_HEAD_BYTES {
    return Err(ParseError::HeadTooLarge);
  }
  let mut storage = Buffer::new(bytes.len(), budget.clone()).map_err(|_| ParseError::OutOfMemory)?;
  storage.write(0, &bytes).map_err(|_| ParseError::OutOfMemory)?;
  Ok(Exchange {
    head: storage.freeze(),
    method: Method::from_bytes(method),
    method_span: 0..method.len() as u32,
    path: path_start..path_end,
    version: 2,
    headers,
    content_length,
    host_index,
    has_body: !end,
    expect_continue,
    keep_alive: true,
  })
}

pub(super) fn closed(http: &mut HttpSocket, socket: u64, events: &mut VecDeque<NativeEvent>) {
  if let Some(h2) = &mut http.h2 {
    for stream in h2.streams.values_mut() {
      if stream.freed {
        continue;
      }
      if !stream.body_ended {
        events.push_back(event(EVENT_BODY_END, 0, socket, stream.exchange, aborted()));
      }
      if let Some(response) = &mut stream.response {
        for result in response.cancel() {
          events.push_back(event(EVENT_PART_SENT, 0, socket, stream.exchange, result));
        }
      }
    }
  }
}

pub(super) fn respond(
  state: &mut DriverState,
  exchange: u64,
  status: u16,
  headers: &[ResponseHeader<'_>],
  body: &[u8],
  body_length: u64,
  streaming: bool,
) -> i32 {
  let Some(entry) = state.http.exchanges.get_mut(&exchange) else {
    return INVALID;
  };
  if entry.responded {
    return INVALID;
  }
  let socket = entry.socket;
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return INVALID;
  };
  let h2 = http.h2.as_mut().unwrap();
  let Some(stream) = h2.streams.get_mut(&entry.stream) else {
    return INVALID;
  };
  if stream.reset {
    return INVALID;
  }
  let result = Response::new(
    &mut h2.connection,
    entry.stream,
    status,
    headers,
    body,
    body_length,
    streaming,
    entry.exchange.method == Method::Head,
    &http.budget,
  );
  let Ok(response) = result else {
    return INVALID;
  };
  stream.response = Some(response);
  entry.responded = true;
  let mut events = VecDeque::new();
  if !stream.body_ended {
    stream.body_ended = true;
    stream.discarded = true;
    events.push_back(event(EVENT_BODY_END, 0, socket, exchange, aborted()));
  }
  pump(state, socket, &mut events);
  state.http.overflow.extend(events);
  0
}

pub(super) fn chunk(state: &mut DriverState, exchange: u64, buffer: u64, length: usize, final_part: bool) -> i32 {
  let Some(entry) = state.http.exchanges.get(&exchange) else {
    return INVALID;
  };
  let socket = entry.socket;
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return INVALID;
  };
  let h2 = http.h2.as_mut().unwrap();
  let Some(stream) = h2.streams.get_mut(&entry.stream) else {
    return INVALID;
  };
  if stream.reset {
    return INVALID;
  }
  let Some(response) = &mut stream.response else {
    return INVALID;
  };
  if !response.accepting() {
    return INVALID;
  }
  if !response.can_accept(length) {
    return BUSY;
  }
  let mut buffers = lock(registry(buffer));
  let Some(Storage::Mutable(storage)) = buffers.get_mut(&buffer) else {
    return INVALID;
  };
  if storage
    .buf_capacity()
    .checked_sub(CHUNK_HEADROOM + CHUNK_TAIL)
    .is_none_or(|max| length > max)
  {
    return INVALID;
  }
  // SAFETY: prepare initialized the full capacity, and the length bound was checked above.
  unsafe {
    storage.set_len(CHUNK_HEADROOM + length);
  }
  let Some(Storage::Mutable(storage)) = buffers.remove(&buffer) else {
    return INVALID;
  };
  drop(buffers);
  let part = FrozenBuffer::slice(&storage.freeze(), CHUNK_HEADROOM..CHUNK_HEADROOM + length).unwrap();
  let result = response.enqueue(part, final_part);
  let mut events = VecDeque::new();
  if result.is_err() {
    h2.connection.reset(entry.stream, ::h2::Reason::PROTOCOL_ERROR);
    stream.reset = true;
    for result in response.cancel() {
      events.push_back(event(EVENT_PART_SENT, 0, socket, exchange, result));
    }
    events.push_back(event(EVENT_RESET, 0, socket, exchange, malformed()));
  }
  pump(state, socket, &mut events);
  state.http.overflow.extend(events);
  // Ownership transferred even for a length mismatch; expose a portable terminal error.
  result.map_or(malformed() as i32, |_| 0)
}

pub(super) fn abandon(state: &mut DriverState, exchange: u64, report: bool) -> i32 {
  let Some(entry) = state.http.exchanges.get_mut(&exchange) else {
    return INVALID;
  };
  entry.responded = true;
  let socket = entry.socket;
  let mut events = VecDeque::new();
  if let Some(h2) = state.http.sockets.get_mut(&socket).and_then(|http| http.h2.as_mut()) {
    h2.connection.reset(entry.stream, ::h2::Reason::CANCEL);
    if let Some(stream) = h2.streams.get_mut(&entry.stream) {
      if stream.reset {
        return 0;
      }
      stream.reset = true;
      if let Some(response) = &mut stream.response {
        for result in response.cancel() {
          if report {
            events.push_back(event(EVENT_PART_SENT, 0, socket, exchange, result));
          }
        }
      }
      if report && !stream.body_ended {
        events.push_back(event(EVENT_BODY_END, 0, socket, exchange, aborted()));
      }
      stream.body_ended = true;
      if report {
        events.push_back(event(EVENT_RESET, 0, socket, exchange, aborted()));
      }
    }
  }
  pump(state, socket, &mut events);
  state.http.overflow.extend(events);
  0
}

pub(super) fn free(state: &mut DriverState, exchange: u64) -> i32 {
  let Some(entry) = state.http.exchanges.get(&exchange) else {
    return INVALID;
  };
  let socket = entry.socket;
  let stream_id = entry.stream;
  state.http.exchanges.retire(exchange, true);
  state.http.overflow.forget_exchange(exchange);
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    state.http.exchanges.recycle(exchange);
    return 0;
  };
  let Some(h2) = &mut http.h2 else {
    state.http.exchanges.recycle(exchange);
    return 0;
  };
  let Some(stream) = h2.streams.get_mut(&stream_id) else {
    state.http.exchanges.recycle(exchange);
    return 0;
  };
  stream.freed = true;
  if stream.response.as_ref().is_none_or(|response| !response.final_queued()) {
    h2.connection.reset(stream_id, ::h2::Reason::CANCEL);
    stream.reset = true;
  }
  http.retired.insert(exchange, ());
  retire(http, &mut state.http.exchanges);
  let mut events = VecDeque::new();
  pump(state, socket, &mut events);
  state.http.overflow.extend(events);
  0
}

/// Authenticated TLS input EOF cannot complete bodies, but accepted responses still drain.
pub(super) fn peer_closed(state: &mut DriverState, socket: u64, events: &mut VecDeque<NativeEvent>) {
  let Some(http) = state.http.sockets.get_mut(&socket) else {
    return;
  };
  let Some(h2) = &mut http.h2 else {
    return;
  };
  http.draining = true;
  h2.draining = true;
  h2.peer_closed = true;
  for stream in h2.streams.values_mut() {
    if !stream.body_ended && !stream.freed {
      events.push_back(event(EVENT_BODY_END, 0, socket, stream.exchange, aborted()));
    }
    stream.body_ended = true;
    stream.discarded = true;
  }
  drive(state, socket, events);
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn normalized_h2_head_preserves_authority_path_and_duplicate_fields() {
    let head = ::http::Request::builder()
      .method("POST")
      .uri("https://example.test:8443/a?q=b")
      .header("x-test", "first")
      .header("x-test", "second")
      .header("content-length", "123")
      .body(())
      .unwrap();
    let parsed = exchange(head, false, &Budget::new(65536)).unwrap();
    assert_eq!(parsed.version, 2);
    assert_eq!(parsed.method_bytes(), b"POST");
    assert_eq!(parsed.path_bytes(), b"/a?q=b");
    assert_eq!(parsed.header(b"host"), Some(b"example.test:8443".as_slice()));
    assert_eq!(parsed.headers.len(), 4);
    assert_eq!(parsed.host_index, Some(0));
    assert_eq!(parsed.content_length, Some(123));
    assert!(parsed.has_body);
  }

  fn request(headers: &[(&str, &str)]) -> ::http::Request<()> {
    let mut builder = ::http::Request::builder().method("PUT").uri("https://example.test/up");
    for (name, value) in headers {
      builder = builder.header(*name, *value);
    }
    builder.body(()).unwrap()
  }

  #[test]
  fn expect_continue_is_armed_only_while_a_body_follows() {
    let budget = Budget::new(65536);
    let head = request(&[("expect", " , 100-Continue")]);
    assert!(exchange(head, false, &budget).unwrap().expect_continue);
    let head = request(&[("expect", "100-continue")]);
    let parsed = exchange(head, true, &budget).unwrap();
    assert!(!parsed.expect_continue);
    assert!(!parsed.has_body);
    let head = request(&[("expect", "100-continue, fancy")]);
    assert_eq!(
      exchange(head, false, &budget).unwrap_err(),
      ParseError::UnsupportedExpectation
    );
  }

  #[test]
  fn explicit_host_field_suppresses_the_authority_and_is_indexed() {
    let head = request(&[("x-first", "1"), ("host", "origin.test"), ("host", "second.test")]);
    let parsed = exchange(head, true, &Budget::new(65536)).unwrap();
    assert_eq!(parsed.method, Method::Put);
    assert_eq!(parsed.headers.len(), 3);
    assert_eq!(parsed.host_index, Some(1));
    assert_eq!(parsed.header(b"host"), Some(b"origin.test".as_slice()));
    assert!(!parsed.head.as_ref().windows(12).any(|w| w == b"example.test"));
  }

  #[test]
  fn first_content_length_wins_and_invalid_values_are_absent() {
    let budget = Budget::new(65536);
    let head = request(&[("content-length", "7"), ("content-length", "9")]);
    assert_eq!(exchange(head, false, &budget).unwrap().content_length, Some(7));
    let head = request(&[("content-length", "seven")]);
    assert_eq!(exchange(head, false, &budget).unwrap().content_length, None);
  }

  #[test]
  fn oversized_or_unaffordable_heads_are_rejected() {
    let budget = Budget::new(1 << 20);
    let many: Vec<(String, &str)> = (0..=MAX_HEADERS).map(|i| (format!("x-{i}"), "v")).collect();
    let many: Vec<(&str, &str)> = many.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    assert_eq!(
      exchange(request(&many), true, &budget).unwrap_err(),
      ParseError::HeadTooLarge
    );
    let large = "a".repeat(MAX_HEAD_BYTES);
    assert_eq!(
      exchange(request(&[("x-large", large.as_str())]), true, &budget).unwrap_err(),
      ParseError::HeadTooLarge
    );
    assert_eq!(
      exchange(request(&[]), true, &Budget::new(8)).unwrap_err(),
      ParseError::OutOfMemory
    );
  }
}
