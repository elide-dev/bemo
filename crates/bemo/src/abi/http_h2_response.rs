/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-stream response admission and physical-write credit for native HTTP/2.

use std::collections::VecDeque;
use std::io;

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};

use crate::abi::{BUSY, INVALID};
use crate::buffer::{Budget, Buffer, FrozenBuffer};
use crate::http::h2::{self, CONTINUATION, DATA, END_HEADERS, END_STREAM, FrameMark, HEADERS, WINDOW};
use crate::http::{DateCache, ResponseHeader};

struct Part {
  bytes: Bytes,
  accepted: usize,
  sent: usize,
  final_part: bool,
  submitted: bool,
}

pub(super) struct Response {
  parts: VecDeque<Part>,
  streaming: bool,
  bodiless: bool,
  declared: Option<u64>,
  body_bytes: u64,
  queued_bytes: usize,
  final_queued: bool,
  header_sent: bool,
  header_wire_bytes: usize,
  wire_ended: bool,
  failed: bool,
  events: VecDeque<i64>,
}

impl Response {
  #[allow(clippy::too_many_arguments)]
  pub(super) fn new(
    connection: &mut h2::Connection,
    stream: u32,
    status: u16,
    headers: &[ResponseHeader<'_>],
    body: &[u8],
    body_length: u64,
    streaming: bool,
    head_only: bool,
    budget: &Budget,
  ) -> io::Result<Self> {
    // The one-shot response ABI cannot represent an informational response followed by a final one.
    if !(200..1000).contains(&status) {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let bodiless = head_only || matches!(status, 204 | 205 | 304);
    let declared = (body_length != u64::MAX).then_some(body_length);
    if !streaming && !bodiless && declared != Some(body.len() as u64) {
      return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut head = http::Response::builder()
      .status(status)
      .body(())
      .map_err(io::Error::other)?;
    for header in headers {
      let name = HeaderName::from_bytes(header.name).map_err(io::Error::other)?;
      let value = HeaderValue::from_bytes(header.value).map_err(io::Error::other)?;
      if matches!(
        name.as_str(),
        "connection"
          | "keep-alive"
          | "proxy-connection"
          | "transfer-encoding"
          | "upgrade"
          | "trailer"
          | "te"
          | "content-length"
          | "date"
          | "server"
      ) {
        continue;
      }
      // Connection-nominated fields are hop-by-hop even if their name is otherwise ordinary.
      if headers.iter().any(|connection| {
        connection.name.eq_ignore_ascii_case(b"connection")
          && connection
            .value
            .split(|byte| *byte == b',')
            .any(|token| token.trim_ascii().eq_ignore_ascii_case(header.name))
      }) {
        continue;
      }
      head.headers_mut().append(name, value);
    }
    if !matches!(status, 204 | 304)
      && let Some(length) = if status == 205 { Some(0) } else { declared }
    {
      head.headers_mut().insert(
        "content-length",
        HeaderValue::from_str(&length.to_string()).map_err(io::Error::other)?,
      );
    }
    head.headers_mut().insert("server", HeaderValue::from_static("elide"));
    let mut date = DateCache::default();
    let line = date.line();
    head.headers_mut().insert(
      "date",
      HeaderValue::from_bytes(&line[6..line.len() - 2]).map_err(io::Error::other)?,
    );
    let mut result = Self {
      parts: VecDeque::new(),
      streaming,
      bodiless,
      declared,
      body_bytes: 0,
      queued_bytes: 0,
      final_queued: !streaming,
      header_sent: false,
      header_wire_bytes: 0,
      wire_ended: false,
      failed: false,
      events: VecDeque::new(),
    };
    // Reserve native storage before committing the response head to h2.
    if !streaming && !bodiless && !body.is_empty() {
      let mut buffer = Buffer::new(body.len(), budget.clone())?;
      buffer.write(0, body)?;
      result.parts.push_back(Part {
        bytes: Bytes::from_owner(buffer.freeze()),
        accepted: 0,
        sent: 0,
        final_part: true,
        submitted: false,
      });
      result.body_bytes = body.len() as u64;
      result.queued_bytes = body.len();
    }
    let end = bodiless || (!streaming && body.is_empty());
    connection.response(stream, head, end)?;
    Ok(result)
  }

  pub(super) fn accepting(&self) -> bool {
    self.streaming && !self.final_queued && !self.failed
  }

  pub(super) fn can_accept(&self, length: usize) -> bool {
    self.streaming
      && !self.final_queued
      && !self.failed
      && self.parts.len() <= WINDOW
      && (self.bodiless || length <= WINDOW.saturating_sub(self.queued_bytes))
  }

  /// Caller preflights before transferring the native buffer. Invalid declared length is terminal.
  pub(super) fn enqueue(&mut self, bytes: FrozenBuffer, final_part: bool) -> Result<(), i32> {
    if !self.streaming || self.final_queued || self.failed {
      return Err(INVALID);
    }
    let length = bytes.as_ref().len();
    if !self.can_accept(length) {
      return Err(BUSY);
    }
    let total = self.body_bytes.checked_add(length as u64).ok_or(INVALID)?;
    if !self.bodiless
      && self
        .declared
        .is_some_and(|declared| total > declared || (final_part && total != declared))
    {
      self.failed = true;
      return Err(INVALID);
    }
    self.body_bytes = total;
    self.final_queued = final_part;
    self.queued_bytes += if self.bodiless { 0 } else { length };
    self.parts.push_back(Part {
      bytes: if self.bodiless {
        Bytes::new()
      } else {
        Bytes::from_owner(bytes)
      },
      accepted: 0,
      sent: 0,
      final_part,
      submitted: self.bodiless,
    });
    self.retire_local();
    Ok(())
  }

  /// Feed queued payload within peer capacity. This operation grants no producer credit.
  pub(super) fn progress(&mut self, connection: &mut h2::Connection, stream: u32) -> io::Result<()> {
    if self.failed {
      return Err(io::ErrorKind::BrokenPipe.into());
    }
    if self.bodiless {
      self.retire_local();
      return Ok(());
    }
    for part in &mut self.parts {
      if part.submitted {
        continue;
      }
      if part.bytes.is_empty() && !part.final_part {
        part.submitted = true;
        continue;
      }
      let remaining = part.bytes.slice(part.accepted..);
      let accepted = connection.data(stream, remaining.clone(), part.final_part)?;
      part.accepted += accepted;
      if accepted < remaining.len() {
        break;
      }
      part.submitted = true;
    }
    self.retire_local();
    Ok(())
  }

  /// The caller invokes this only after the frame's last byte reaches the physical transport.
  pub(super) fn acknowledge(&mut self, mark: &FrameMark) {
    if self.failed {
      return;
    }
    match mark.kind {
      HEADERS | CONTINUATION => {
        self.header_wire_bytes += 9 + mark.payload_bytes;
        if mark.flags & END_HEADERS != 0 {
          self.header_sent = true;
          if self.streaming {
            self.events.push_back(self.header_wire_bytes as i64);
          }
        }
        if mark.kind == HEADERS && mark.flags & END_STREAM != 0 {
          self.wire_ended = true;
        }
      }
      DATA => {
        let mut left = mark.payload_bytes;
        for part in &mut self.parts {
          let n = left.min(part.bytes.len() - part.sent);
          part.sent += n;
          left -= n;
          if left == 0 {
            break;
          }
        }
        if mark.flags & END_STREAM != 0 {
          self.wire_ended = true;
        }
      }
      _ => {}
    }
    self.retire_local();
  }

  fn retire_local(&mut self) {
    if !self.header_sent {
      return;
    }
    while let Some(part) = self.parts.front() {
      if !part.submitted || part.sent != part.bytes.len() || (part.final_part && !self.wire_ended) {
        break;
      }
      let part = self.parts.pop_front().unwrap();
      self.queued_bytes -= part.bytes.len();
      // HTTP/2 DATA credit is payload-only; frame overhead has its own bounded wire queue.
      if self.streaming {
        self.events.push_back(part.bytes.len() as i64);
      }
    }
  }

  pub(super) fn take_events(&mut self) -> Vec<i64> {
    self.events.drain(..).collect()
  }

  pub(super) fn wire_complete(&self) -> bool {
    self.header_sent && self.wire_ended && self.final_queued && self.parts.is_empty()
  }

  pub(super) fn final_queued(&self) -> bool {
    self.final_queued
  }

  /// Fail every outstanding report exactly once; already-earned successful credits stay ordered.
  pub(super) fn cancel(&mut self) -> Vec<i64> {
    self.failed = true;
    let error = super::super::aborted();
    if self.streaming {
      if !self.header_sent {
        self.events.push_back(error);
        self.header_sent = true;
      }
      self.events.extend(std::iter::repeat_n(error, self.parts.len()));
    }
    self.parts.clear();
    self.queued_bytes = 0;
    self.events.drain(..).collect()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::http::h2::tests::Pair;
  use std::future::Future;
  use std::pin::Pin;
  use std::task::{Context, Poll, Waker};

  fn buffer(length: usize) -> FrozenBuffer {
    let mut buffer = Buffer::new(length.max(1), Budget::new(length.max(1))).unwrap();
    buffer.write(0, &vec![42; length]).unwrap();
    buffer.freeze()
  }

  fn fixture() -> (Pair, u32, ::h2::client::ResponseFuture) {
    fixture_method("GET")
  }

  fn fixture_method(method: &str) -> (Pair, u32, ::h2::client::ResponseFuture) {
    let mut pair = Pair::new(WINDOW as u32);
    let (response, _) = pair
      .sender
      .send_request(
        http::Request::builder()
          .method(method)
          .uri("https://localhost/")
          .body(())
          .unwrap(),
        true,
      )
      .unwrap();
    let id = pair.requests(1)[0].0;
    (pair, id, response)
  }

  fn streaming(pair: &mut Pair, id: u32, length: u64) -> Response {
    Response::new(
      &mut pair.server,
      id,
      200,
      &[],
      &[],
      length,
      true,
      false,
      &Budget::new(WINDOW),
    )
    .unwrap()
  }

  #[test]
  fn streaming_credit_waits_for_physical_frame_acknowledgements() {
    let (mut pair, id, _response) = fixture();
    let mut response = streaming(&mut pair, id, 32768);
    response.enqueue(buffer(32768), true).unwrap();
    for _ in 0..8 {
      response.progress(&mut pair.server, id).unwrap();
      pair.tick();
    }
    assert!(response.take_events().is_empty());
    assert!(!response.wire_complete());
    let marks = std::mem::take(&mut pair.marks);
    let mut data_frames = 0;
    for mark in marks {
      if mark.stream != id {
        continue;
      }
      response.acknowledge(&mark);
      if mark.kind == HEADERS {
        assert!(response.take_events().first().is_some_and(|size| *size > 0));
      } else if mark.kind == DATA {
        data_frames += 1;
        if mark.flags & END_STREAM == 0 {
          assert!(response.take_events().is_empty());
        }
      }
    }
    assert!(data_frames >= 2);
    assert_eq!(response.take_events(), [32768]);
    assert!(response.wire_complete());
  }

  #[test]
  fn window_is_held_until_send_and_empty_final_waits_for_its_frame() {
    let (mut pair, id, _response) = fixture();
    let mut response = streaming(&mut pair, id, u64::MAX);
    response.enqueue(buffer(WINDOW), false).unwrap();
    assert!(!response.can_accept(1));
    assert_eq!(response.enqueue(buffer(1), false), Err(BUSY));
    // A final empty part needs no payload credit even with a completely full window.
    response.enqueue(buffer(0), true).unwrap();
    assert!(!response.wire_complete());
    let cancelled = response.cancel();
    assert_eq!(cancelled.len(), 3); // Head plus both accepted parts.
    assert!(cancelled.iter().all(|result| *result < 0));
    assert!(response.cancel().is_empty());
  }

  #[test]
  fn declared_length_divergence_rejects_underflow_and_overflow() {
    for (length, final_part) in [(5, false), (3, true)] {
      let (mut pair, id, _response) = fixture();
      let mut response = streaming(&mut pair, id, 4);
      assert_eq!(response.enqueue(buffer(length), final_part), Err(INVALID));
      assert!(!response.can_accept(1));
      assert!(!response.wire_complete());
    }
  }

  #[test]
  fn head_and_bodiless_statuses_end_on_headers_and_credit_synthetic_final() {
    for (status, head_only) in [(200, true), (204, false), (205, false), (304, false)] {
      let (mut pair, id, mut incoming) = fixture_method(if head_only { "HEAD" } else { "GET" });
      let mut response = Response::new(
        &mut pair.server,
        id,
        status,
        &[],
        &[],
        10,
        true,
        head_only,
        &Budget::new(WINDOW),
      )
      .unwrap();
      for _ in 0..8 {
        pair.tick();
      }
      for mark in std::mem::take(&mut pair.marks) {
        response.acknowledge(&mark);
      }
      assert_eq!(response.take_events().len(), 1);
      assert!(!response.wire_complete());
      response.enqueue(buffer(0), true).unwrap();
      assert_eq!(response.take_events(), [0]);
      assert!(response.wire_complete());
      let mut cx = Context::from_waker(Waker::noop());
      let Poll::Ready(Ok(reply)) = Pin::new(&mut incoming).poll(&mut cx) else {
        panic!("response absent")
      };
      assert_eq!(reply.status(), status);
      if head_only {
        assert_eq!(reply.headers()["content-length"], "10");
      }
      if matches!(status, 204 | 304) {
        assert!(!reply.headers().contains_key("content-length"));
      }
      if status == 205 {
        assert_eq!(reply.headers()["content-length"], "0");
      }
      assert!(reply.body().is_end_stream());
    }
  }

  #[test]
  fn connection_specific_headers_are_removed_and_buffered_responses_do_not_report_parts() {
    let (mut pair, id, mut incoming) = fixture();
    let headers = [
      ResponseHeader {
        name: b"Connection",
        value: b"x-secret",
      },
      ResponseHeader {
        name: b"X-Secret",
        value: b"hop-only",
      },
      ResponseHeader {
        name: b"Transfer-Encoding",
        value: b"chunked",
      },
      ResponseHeader {
        name: b"X-Visible",
        value: b"yes",
      },
    ];
    let mut response = Response::new(
      &mut pair.server,
      id,
      200,
      &headers,
      b"hello",
      5,
      false,
      false,
      &Budget::new(WINDOW),
    )
    .unwrap();
    for _ in 0..8 {
      response.progress(&mut pair.server, id).unwrap();
      pair.tick();
    }
    for mark in std::mem::take(&mut pair.marks) {
      response.acknowledge(&mark);
    }
    assert!(response.take_events().is_empty());
    assert!(response.wire_complete());
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Ok(reply)) = Pin::new(&mut incoming).poll(&mut cx) else {
      panic!("response absent")
    };
    assert!(!reply.headers().contains_key("connection"));
    assert!(!reply.headers().contains_key("x-secret"));
    assert!(!reply.headers().contains_key("transfer-encoding"));
    assert_eq!(reply.headers()["x-visible"], "yes");
    assert_eq!(reply.headers()["content-length"], "5");
  }
}
