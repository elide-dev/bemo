/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Incremental HTTP/1.x request parsing over received buffers.
//!
//! A [`Connection`] is fed each receive completion's [`Buffer`]. Complete heads
//! that arrive inside one receive are sliced out of that storage without a
//! copy; a head split across receives is assembled in a small accumulator and
//! copied once. Body bytes (`Content-Length` or `chunked`) are never copied:
//! each receive yields at most one [`Outcome::Segment`] slice per body part,
//! followed by [`Outcome::BodyEnd`] once the framing completes.

use std::collections::VecDeque;
use std::ops::Range;

use ntex_httparse::{Header, HeaderParsed, Request as HeadParser, SlicePos, Status, parse_chunk_size};

use super::{MAX_CHUNK_LINE_BYTES, MAX_HEAD_BYTES, MAX_HEADERS};
use crate::buffer::{Budget, Buffer, FrozenBuffer};

/// Request method as a small code shared with the JVM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Method {
  Get = 0,
  Head = 1,
  Post = 2,
  Put = 3,
  Delete = 4,
  Connect = 5,
  Options = 6,
  Trace = 7,
  Patch = 8,
  Other = 255,
}

impl Method {
  pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
    match bytes {
      b"GET" => Self::Get,
      b"HEAD" => Self::Head,
      b"POST" => Self::Post,
      b"PUT" => Self::Put,
      b"DELETE" => Self::Delete,
      b"CONNECT" => Self::Connect,
      b"OPTIONS" => Self::Options,
      b"TRACE" => Self::Trace,
      b"PATCH" => Self::Patch,
      _ => Self::Other,
    }
  }
}

/// Byte ranges of one header within [`Exchange::head`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderSpan {
  pub name: Range<u32>,
  pub value: Range<u32>,
}

/// A parsed request whose bytes live in native storage for the exchange lifetime.
#[derive(Debug)]
pub struct Exchange {
  /// The request head bytes (request line through the blank line).
  pub head: FrozenBuffer,
  pub method: Method,
  pub method_span: Range<u32>,
  pub path: Range<u32>,
  /// `0` for HTTP/1.0, `1` for HTTP/1.1.
  pub version: u8,
  pub headers: Vec<HeaderSpan>,
  /// Validated declared length, before receiving any body bytes.
  pub content_length: Option<u64>,
  /// First Host field in wire order.
  pub host_index: Option<u32>,
  /// Whether [`Outcome::Segment`]s and an [`Outcome::BodyEnd`] follow this request.
  pub has_body: bool,
  /// Send an interim response before waiting for this HTTP/1.1 body.
  pub expect_continue: bool,
  /// Whether the connection may carry another request after the response.
  pub keep_alive: bool,
}

impl Exchange {
  pub fn method_bytes(&self) -> &[u8] {
    &self.head.as_ref()[range(&self.method_span)]
  }

  pub fn path_bytes(&self) -> &[u8] {
    &self.head.as_ref()[range(&self.path)]
  }

  pub fn header_name(&self, index: usize) -> Option<&[u8]> {
    self
      .headers
      .get(index)
      .map(|span| &self.head.as_ref()[range(&span.name)])
  }

  pub fn header_value(&self, index: usize) -> Option<&[u8]> {
    self
      .headers
      .get(index)
      .map(|span| &self.head.as_ref()[range(&span.value)])
  }

  /// First header with `name`, compared case-insensitively.
  pub fn header(&self, name: &[u8]) -> Option<&[u8]> {
    (0..self.headers.len())
      .find(|&i| self.header_name(i).is_some_and(|n| n.eq_ignore_ascii_case(name)))
      .and_then(|i| self.header_value(i))
  }
}

fn range(r: &Range<u32>) -> Range<usize> {
  r.start as usize..r.end as usize
}

/// Why a request could not be parsed; each maps to the status the connection answers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
  /// Malformed request line, header, or chunk framing.
  BadRequest,
  /// Head or trailers exceeded [`MAX_HEAD_BYTES`] or [`MAX_HEADERS`].
  HeadTooLarge,
  /// A transfer coding other than a single final `chunked`.
  UnsupportedTransferEncoding,
  /// An expectation other than `100-continue`.
  UnsupportedExpectation,
  /// Storage could not be reserved for the head.
  OutOfMemory,
}

impl ParseError {
  pub const fn status(self) -> u16 {
    match self {
      Self::BadRequest => 400,
      Self::HeadTooLarge => 431,
      Self::UnsupportedTransferEncoding => 501,
      Self::UnsupportedExpectation => 417,
      Self::OutOfMemory => 503,
    }
  }
}

/// Result of feeding one receive to a connection.
///
/// A request with [`Exchange::has_body`] is followed by zero or more segments and
/// exactly one `BodyEnd`; one without emits neither.
#[derive(Debug)]
pub enum Outcome {
  Request(Exchange),
  /// Body bytes, a zero-copy slice of the receive they arrived in.
  Segment(FrozenBuffer),
  /// The current request's body framing is complete.
  BodyEnd,
  /// The connection must answer with this status and close.
  Error(ParseError),
}

/// Scratch for a chunk-size line split across receives: 16 hex digits, LWS, CRLF.
const CHUNK_LINE: usize = 24;

#[derive(Debug)]
enum Body {
  Length {
    remaining: u64,
  },
  ChunkSize {
    line: [u8; CHUNK_LINE],
    len: u8,
  },
  ChunkExt {
    size: u64,
    len: usize,
  },
  /// `\n` closing a chunk-size line that carried an extension.
  ExtLf {
    size: u64,
  },
  ChunkData {
    remaining: u64,
  },
  ChunkCr,
  ChunkLf,
  Trailers,
}

impl Body {
  fn chunk_size() -> Self {
    Self::ChunkSize {
      line: [0; CHUNK_LINE],
      len: 0,
    }
  }

  fn after_size(size: u64) -> Self {
    if size == 0 {
      Self::Trailers
    } else {
      Self::ChunkData { remaining: size }
    }
  }
}

/// Per-connection parser state.
#[derive(Debug)]
pub struct Connection {
  budget: Budget,
  /// Bytes of an incomplete head or trailer block carried across receives.
  partial: Vec<u8>,
  head_progress: HeadProgress,
  body: Option<Body>,
  /// Consume the current body without emitting segments or its end.
  discard: bool,
  /// Set once a request asked to close or an error occurred; no further head is parsed.
  closed: bool,
}

impl Connection {
  pub fn new(budget: Budget) -> Self {
    Self {
      budget,
      partial: Vec::new(),
      head_progress: HeadProgress::default(),
      body: None,
      discard: false,
      closed: false,
    }
  }

  pub fn is_closed(&self) -> bool {
    self.closed
  }

  /// Whether a request body is still being framed.
  pub fn body_pending(&self) -> bool {
    self.body.is_some()
  }

  /// Drop the rest of the current body: framing continues, nothing more is emitted for it.
  pub fn discard_body(&mut self) {
    self.discard = self.body.is_some();
  }

  /// Consume one receive completion, appending requests, body segments and errors to `out`.
  ///
  /// After an [`Outcome::Error`] the connection is closed and later input is ignored.
  pub fn ingest(&mut self, received: Buffer, out: &mut VecDeque<Outcome>) {
    self.ingest_bounded(received.freeze(), usize::MAX, out);
  }

  /// Parse up to `heads` new requests, continuing the current body even without head credit.
  /// Returns unconsumed storage when admission stops at a request boundary.
  pub fn ingest_bounded(
    &mut self,
    frozen: FrozenBuffer,
    mut heads: usize,
    out: &mut VecDeque<Outcome>,
  ) -> Option<FrozenBuffer> {
    if self.closed && self.body.is_none() {
      return None;
    }
    let bytes = frozen.as_ref();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
      if self.body.is_some() {
        match self.body_step(&frozen, cursor, out) {
          Ok(next) => cursor = next,
          Err(error) => {
            self.fail(error, out);
            return None;
          }
        }
        continue;
      }
      if self.closed {
        return None;
      }
      if heads == 0 {
        return FrozenBuffer::slice(&frozen, cursor..bytes.len()).ok();
      }
      if self.partial.is_empty() {
        // Fast path: the head starts at `cursor` inside this receive.
        let mut progress = HeadProgress::default();
        match progress.parse(&bytes[cursor..]) {
          Ok(Status::Complete(head)) => {
            let Ok(slice) = FrozenBuffer::slice(&frozen, cursor..cursor + head.len) else {
              self.fail(ParseError::OutOfMemory, out);
              return None;
            };
            cursor += head.len;
            heads -= 1;
            if let Err(error) = self.finish(head, slice, out) {
              self.fail(error, out);
              return None;
            }
          }
          Ok(Status::Partial) => {
            self.head_progress = progress;
            if bytes.len() - cursor > MAX_HEAD_BYTES {
              self.fail(ParseError::HeadTooLarge, out);
              return None;
            }
            self.partial.reserve((bytes.len() - cursor).max(128));
            self.partial.extend_from_slice(&bytes[cursor..]);
            return None;
          }
          Err(error) => {
            self.fail(error, out);
            return None;
          }
        }
      } else {
        // Slow path: complete a head that began in an earlier receive.
        let room = MAX_HEAD_BYTES.saturating_sub(self.partial.len());
        let take = room.min(bytes.len() - cursor);
        self.partial.extend_from_slice(&bytes[cursor..cursor + take]);
        match self.head_progress.parse(&self.partial) {
          Ok(Status::Complete(head)) => {
            // Bytes past the head belong to the body or the next request; only
            // the head itself is copied into owned storage.
            let consumed_now = head.len - (self.partial.len() - take);
            cursor += consumed_now;
            let mut storage = match Buffer::new(head.len, self.budget.clone()) {
              Ok(storage) => storage,
              Err(_) => {
                self.fail(ParseError::OutOfMemory, out);
                return None;
              }
            };
            if storage.write(0, &self.partial[..head.len]).is_err() {
              self.fail(ParseError::OutOfMemory, out);
              return None;
            }
            self.partial.clear();
            heads -= 1;
            if let Err(error) = self.finish(head, storage.freeze(), out) {
              self.fail(error, out);
              return None;
            }
          }
          Ok(Status::Partial) => {
            if take < bytes.len() - cursor || self.partial.len() >= MAX_HEAD_BYTES {
              self.fail(ParseError::HeadTooLarge, out);
            }
            return None;
          }
          Err(error) => {
            self.fail(error, out);
            return None;
          }
        }
      }
    }
    None
  }

  /// Advance body framing over `frozen[cursor..]`; returns the new cursor.
  fn body_step(
    &mut self,
    frozen: &FrozenBuffer,
    cursor: usize,
    out: &mut VecDeque<Outcome>,
  ) -> Result<usize, ParseError> {
    let bytes = frozen.as_ref();
    let rest = &bytes[cursor..];
    let body = self.body.take().expect("body pending");
    let (next, cursor) = match body {
      Body::Length { remaining } | Body::ChunkData { remaining } => {
        let take = usize::try_from(remaining).map_or(rest.len(), |r| r.min(rest.len()));
        if !self.discard {
          let segment = frozen
            .slice(cursor..cursor + take)
            .map_err(|_| ParseError::OutOfMemory)?;
          out.push_back(Outcome::Segment(segment));
        }
        let remaining = remaining - take as u64;
        let next = match (remaining > 0, matches!(body, Body::Length { .. })) {
          (true, true) => Some(Body::Length { remaining }),
          (true, false) => Some(Body::ChunkData { remaining }),
          (false, true) => None,
          (false, false) => Some(Body::ChunkCr),
        };
        (next, cursor + take)
      }
      Body::ChunkSize { mut line, len } => {
        let len = len as usize;
        let room = CHUNK_LINE - 2 - len;
        let window = &rest[..rest.len().min(room)];
        match window.iter().position(|b| *b == b'\n' || *b == b';') {
          None if rest.len() > room => return Err(ParseError::BadRequest),
          None => {
            line[len..len + window.len()].copy_from_slice(window);
            let len = (len + window.len()) as u8;
            (Some(Body::ChunkSize { line, len }), bytes.len())
          }
          Some(i) => {
            let ends_line = window[i] == b'\n';
            let size = if len == 0 && ends_line {
              chunk_size(&window[..=i])?
            } else {
              line[len..len + i].copy_from_slice(&window[..i]);
              let mut end = len + i;
              if ends_line {
                line[end] = b'\n';
                end += 1;
              } else {
                line[end..end + 2].copy_from_slice(b"\r\n");
                end += 2;
              }
              chunk_size(&line[..end])?
            };
            let next = if ends_line {
              Body::after_size(size)
            } else {
              Body::ChunkExt { size, len: 0 }
            };
            (Some(next), cursor + i + 1)
          }
        }
      }
      Body::ChunkExt { size, len } => {
        let room = MAX_CHUNK_LINE_BYTES - len;
        let window = &rest[..rest.len().min(room)];
        match window.iter().position(|b| *b == b'\r' || *b == b'\n') {
          Some(i) if window[i] == b'\r' => (Some(Body::ExtLf { size }), cursor + i + 1),
          Some(_) => return Err(ParseError::BadRequest),
          None if rest.len() > room => return Err(ParseError::BadRequest),
          None => {
            let len = len + window.len();
            (Some(Body::ChunkExt { size, len }), bytes.len())
          }
        }
      }
      Body::ExtLf { size } if rest[0] == b'\n' => (Some(Body::after_size(size)), cursor + 1),
      Body::ChunkCr if rest[0] == b'\r' => (Some(Body::ChunkLf), cursor + 1),
      Body::ChunkLf if rest[0] == b'\n' => (Some(Body::chunk_size()), cursor + 1),
      Body::ExtLf { .. } | Body::ChunkCr | Body::ChunkLf => return Err(ParseError::BadRequest),
      // Trailers are parsed for framing only and dropped; nothing consumes them.
      Body::Trailers if self.partial.is_empty() => match parse_headers(rest, 0, &mut |_| {})? {
        Status::Complete(len) => (None, cursor + len),
        Status::Partial => {
          self.partial.extend_from_slice(rest);
          (Some(Body::Trailers), bytes.len())
        }
      },
      Body::Trailers => {
        let room = MAX_HEAD_BYTES.saturating_sub(self.partial.len());
        let take = room.min(rest.len());
        self.partial.extend_from_slice(&rest[..take]);
        match parse_headers(&self.partial, 0, &mut |_| {})? {
          Status::Complete(len) => {
            let consumed_now = len - (self.partial.len() - take);
            self.partial.clear();
            (None, cursor + consumed_now)
          }
          Status::Partial if take < rest.len() || self.partial.len() >= MAX_HEAD_BYTES => {
            return Err(ParseError::HeadTooLarge);
          }
          Status::Partial => (Some(Body::Trailers), bytes.len()),
        }
      }
    };
    match next {
      Some(body) => self.body = Some(body),
      None => {
        if !self.discard {
          out.push_back(Outcome::BodyEnd);
        }
        self.discard = false;
      }
    }
    Ok(cursor)
  }

  /// Validate framing headers for a complete head, emit the request, and arm its body.
  fn finish(&mut self, head: Head, slice: FrozenBuffer, out: &mut VecDeque<Outcome>) -> Result<(), ParseError> {
    let mut exchange = Exchange {
      head: slice,
      method: head.method,
      method_span: head.method_span,
      path: head.path,
      version: head.version,
      headers: head.headers,
      content_length: None,
      host_index: None,
      has_body: false,
      expect_continue: false,
      keep_alive: head.keep_alive,
    };
    let mut length = None;
    let mut transfer_encoding = false;
    let mut unsupported_coding = false;
    let mut chunked = false;
    let mut expect_continue = false;
    for index in 0..exchange.headers.len() {
      let Some(name) = exchange.header_name(index) else {
        continue;
      };
      if name.eq_ignore_ascii_case(b"host") {
        exchange.host_index.get_or_insert(index as u32);
      } else if name.eq_ignore_ascii_case(b"content-length") {
        match exchange.header_value(index).and_then(parse_content_length) {
          Some(value) if length.is_none_or(|previous| previous == value) => length = Some(value),
          _ => return Err(ParseError::BadRequest),
        }
      } else if exchange.version != 0 && name.eq_ignore_ascii_case(b"expect") {
        for expectation in exchange.header_value(index).unwrap_or_default().split(|b| *b == b',') {
          let expectation = expectation.trim_ascii();
          if expectation.is_empty() {
            continue;
          }
          if !expectation.eq_ignore_ascii_case(b"100-continue") {
            return Err(ParseError::UnsupportedExpectation);
          }
          expect_continue = true;
        }
      } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
        transfer_encoding = true;
        let value = exchange.header_value(index).unwrap_or_default();
        for coding in value.split(|b| *b == b',') {
          let coding = coding.trim_ascii();
          if coding.is_empty() {
            continue;
          }
          // Only a single, final `chunked` is understood; any other coding is 501.
          if chunked || !coding.eq_ignore_ascii_case(b"chunked") {
            unsupported_coding = true;
            break;
          }
          chunked = true;
        }
      }
    }
    if transfer_encoding {
      if length.is_some() || exchange.version == 0 {
        return Err(ParseError::BadRequest);
      }
      if unsupported_coding || !chunked {
        return Err(ParseError::UnsupportedTransferEncoding);
      }
    }
    let body = match length {
      _ if chunked => Some(Body::chunk_size()),
      Some(remaining) if remaining > 0 => Some(Body::Length { remaining }),
      _ => None,
    };
    exchange.content_length = length;
    exchange.has_body = body.is_some();
    exchange.expect_continue = expect_continue && exchange.has_body;
    if !exchange.keep_alive {
      self.closed = true;
    }
    out.push_back(Outcome::Request(exchange));
    self.body = body;
    Ok(())
  }

  fn fail(&mut self, error: ParseError, out: &mut VecDeque<Outcome>) {
    self.closed = true;
    self.partial.clear();
    self.head_progress = HeadProgress::default();
    self.body = None;
    self.discard = false;
    out.push_back(Outcome::Error(error));
  }
}

/// Parse one complete chunk-size line (through CRLF), extensions excluded.
fn chunk_size(line: &[u8]) -> Result<u64, ParseError> {
  if !line.first().is_some_and(u8::is_ascii_hexdigit) {
    return Err(ParseError::BadRequest);
  }
  match parse_chunk_size(line) {
    Ok(Status::Complete((_, size))) => Ok(size),
    _ => Err(ParseError::BadRequest),
  }
}

#[derive(Debug)]
struct Head {
  len: usize,
  method: Method,
  method_span: Range<u32>,
  path: Range<u32>,
  version: u8,
  headers: Vec<HeaderSpan>,
  keep_alive: bool,
}

fn span(pos: SlicePos, base: usize) -> Range<u32> {
  (pos.start + base) as u32..(pos.end + base) as u32
}

/// Cache validated complete lines while retaining strict parsing of the current incomplete line.
#[derive(Debug, Default)]
struct HeadProgress {
  head: Option<Head>,
  cursor: usize,
  close: bool,
  keep_alive: bool,
}

impl HeadProgress {
  #[inline]
  fn parse(&mut self, src: &[u8]) -> Result<Status<Head>, ParseError> {
    if self.head.is_none() {
      let mut line = HeadParser::default();
      self.cursor = match line.parse(src) {
        Ok(Status::Complete(len)) => len,
        Ok(Status::Partial) => return Ok(Status::Partial),
        Err(_) => return Err(ParseError::BadRequest),
      };
      self.head = Some(Head {
        len: 0,
        method: Method::from_bytes(&src[line.method.start..line.method.end]),
        method_span: span(line.method, 0),
        path: span(line.path, 0),
        version: line.version,
        headers: Vec::new(),
        keep_alive: false,
      });
    }
    let head = self.head.as_mut().unwrap();
    loop {
      if head.headers.len() > MAX_HEADERS {
        return Err(ParseError::HeadTooLarge);
      }
      let mut header = Header::default();
      match header.parse(&src[self.cursor..]) {
        Ok(Status::Complete(HeaderParsed::Header(len))) => {
          let field = HeaderSpan {
            name: span(header.name, self.cursor),
            value: span(header.value, self.cursor),
          };
          if src[range(&field.name)].eq_ignore_ascii_case(b"connection") {
            let value = &src[range(&field.value)];
            self.close |= token_present(value, b"close");
            self.keep_alive |= token_present(value, b"keep-alive");
          }
          head.headers.push(field);
          self.cursor += len;
        }
        Ok(Status::Complete(HeaderParsed::Eof(len))) => {
          self.cursor += len;
          if self.cursor > MAX_HEAD_BYTES {
            return Err(ParseError::HeadTooLarge);
          }
          let mut head = self.head.take().unwrap();
          head.len = self.cursor;
          head.keep_alive = !self.close && (head.version != 0 || self.keep_alive);
          *self = Self::default();
          return Ok(Status::Complete(head));
        }
        Ok(Status::Partial) => {
          return if src.len() > MAX_HEAD_BYTES {
            Err(ParseError::HeadTooLarge)
          } else {
            Ok(Status::Partial)
          };
        }
        Err(ntex_httparse::Error::TooManyHeaders) => return Err(ParseError::HeadTooLarge),
        Err(_) => return Err(ParseError::BadRequest),
      }
    }
  }
}

/// Parse a header block starting at `start` through its blank line; returns the end offset.
fn parse_headers(src: &[u8], start: usize, sink: &mut dyn FnMut(HeaderSpan)) -> Result<Status<usize>, ParseError> {
  let mut cursor = start;
  let mut count = 0usize;
  loop {
    if count > MAX_HEADERS {
      return Err(ParseError::HeadTooLarge);
    }
    let mut header = Header::default();
    match header.parse(&src[cursor..]) {
      Ok(Status::Complete(HeaderParsed::Header(len))) => {
        sink(HeaderSpan {
          name: span(header.name, cursor),
          value: span(header.value, cursor),
        });
        count += 1;
        cursor += len;
      }
      Ok(Status::Complete(HeaderParsed::Eof(len))) => {
        cursor += len;
        break;
      }
      Ok(Status::Partial) => {
        return if src.len() > MAX_HEAD_BYTES {
          Err(ParseError::HeadTooLarge)
        } else {
          Ok(Status::Partial)
        };
      }
      Err(ntex_httparse::Error::TooManyHeaders) => return Err(ParseError::HeadTooLarge),
      Err(_) => return Err(ParseError::BadRequest),
    }
  }
  if cursor > MAX_HEAD_BYTES {
    return Err(ParseError::HeadTooLarge);
  }
  Ok(Status::Complete(cursor))
}

fn token_present(list: &[u8], token: &[u8]) -> bool {
  list
    .split(|b| *b == b',')
    .any(|item| item.trim_ascii().eq_ignore_ascii_case(token))
}

fn parse_content_length(value: &[u8]) -> Option<u64> {
  let value = value.trim_ascii();
  if value.is_empty() || value.len() > 19 {
    return None;
  }
  value.iter().try_fold(0u64, |acc, b| {
    b.is_ascii_digit().then(|| acc * 10 + u64::from(b - b'0'))
  })
}

#[cfg(test)]
mod tests {
  use compio_buf::IoBuf;

  use super::*;

  fn budget() -> Budget {
    Budget::new(1 << 20)
  }

  fn reference_head(src: &[u8]) -> Result<Status<Head>, ParseError> {
    let mut line = HeadParser::default();
    let line_len = match line.parse(src) {
      Ok(Status::Complete(len)) => len,
      Ok(Status::Partial) => return Ok(Status::Partial),
      Err(_) => return Err(ParseError::BadRequest),
    };
    let mut headers = Vec::new();
    let mut close = false;
    let mut keep_alive = false;
    let parsed = parse_headers(src, line_len, &mut |span| {
      if src[range(&span.name)].eq_ignore_ascii_case(b"connection") {
        let value = &src[range(&span.value)];
        close |= token_present(value, b"close");
        keep_alive |= token_present(value, b"keep-alive");
      }
      headers.push(span);
    })?;
    let Status::Complete(cursor) = parsed else {
      return Ok(Status::Partial);
    };
    let version = line.version;
    let keep_alive = !close && (version != 0 || keep_alive);
    Ok(Status::Complete(Head {
      len: cursor,
      method: Method::from_bytes(&src[line.method.start..line.method.end]),
      method_span: span(line.method, 0),
      path: span(line.path, 0),
      version,
      headers,
      keep_alive,
    }))
  }

  #[test]
  #[cfg_attr(
    miri,
    ignore = "exhaustive large-head prefix oracle is covered by native tests and fuzzing"
  )]
  fn incremental_heads_match_reference_at_every_prefix() {
    let many = format!("GET / HTTP/1.1\r\n{}\r\n", "x-a: v\r\n".repeat(MAX_HEADERS + 1));
    let large = format!("GET / HTTP/1.1\r\nx-a: {}", "a".repeat(MAX_HEAD_BYTES));
    for wire in [
      b"GET /p?q=1 HTTP/1.1\r\nHost: a\r\nConnection: keep-alive, close\r\n\r\n".as_slice(),
      b"POST / HTTP/1.0\nConnection: keep-alive\nContent-Length: 4\n\nbody".as_slice(),
      b"G\0T / HTTP/1.1\r\n\r\n".as_slice(),
      b"GET / HTTP/1.1\r\nbad name: v\r\n\r\n".as_slice(),
      b"GET / HTTP/1.1\r\nx-a: bad\0value\r\n\r\n".as_slice(),
      many.as_bytes(),
      large.as_bytes(),
    ] {
      let mut progress = HeadProgress::default();
      for end in 0..=wire.len() {
        let source = &wire[..end];
        match (reference_head(source), progress.parse(source)) {
          (Ok(Status::Partial), Ok(Status::Partial)) => {}
          (Err(expected), Err(actual)) => {
            assert_eq!(actual, expected);
            break;
          }
          (Ok(Status::Complete(expected)), Ok(Status::Complete(actual))) => {
            assert_eq!(actual.len, expected.len);
            assert_eq!(actual.method, expected.method);
            assert_eq!(actual.method_span, expected.method_span);
            assert_eq!(actual.path, expected.path);
            assert_eq!(actual.version, expected.version);
            assert_eq!(actual.headers, expected.headers);
            assert_eq!(actual.keep_alive, expected.keep_alive);
            assert!(progress.head.is_none());
            break;
          }
          (expected, actual) => panic!("prefix {end}: {actual:?}, expected {expected:?}"),
        }
      }
    }
  }

  #[test]
  fn fragmented_pipeline_preserves_spans_and_current_line_early_errors() {
    let wire = b"POST /one HTTP/1.1\r\nHost: a\r\nContent-Length: 4\r\n\r\nbodyGET /two HTTP/1.1\r\nHost: b\r\n\r\n";
    for fragment in 1..=wire.len() {
      let mut connection = Connection::new(budget());
      let mut events = Vec::new();
      for chunk in wire.chunks(fragment) {
        events.extend(feed(&mut connection, chunk));
      }
      let requests: Vec<_> = events
        .iter()
        .filter_map(|event| {
          if let Outcome::Request(request) = event {
            Some(request)
          } else {
            None
          }
        })
        .collect();
      assert_eq!(requests.len(), 2);
      assert_eq!(requests[0].path_bytes(), b"/one");
      assert_eq!(requests[1].path_bytes(), b"/two");
      assert_eq!(requests[0].header(b"host"), Some(b"a".as_slice()));
      assert_eq!(requests[1].header(b"host"), Some(b"b".as_slice()));
      assert_eq!(
        events.iter().filter(|event| matches!(event, Outcome::BodyEnd)).count(),
        1
      );
      let body: Vec<_> = events
        .iter()
        .filter_map(|event| {
          if let Outcome::Segment(part) = event {
            Some(part.as_ref())
          } else {
            None
          }
        })
        .flatten()
        .copied()
        .collect();
      assert_eq!(body, b"body");
      assert!(events.iter().all(|event| !matches!(event, Outcome::Error(_))));
    }
    let mut connection = Connection::new(budget());
    assert!(feed(&mut connection, b"GET / HTTP/1.1\r\nHost: a\r\nx-name:").is_empty());
    assert_eq!(error(&feed(&mut connection, b" bad\0")[0]), ParseError::BadRequest);
    assert!(connection.head_progress.head.is_none());
  }

  fn buffer(bytes: &[u8]) -> Buffer {
    let mut b = Buffer::new(bytes.len().max(1), budget()).unwrap();
    b.write(0, bytes).unwrap();
    b
  }

  fn feed(conn: &mut Connection, bytes: &[u8]) -> Vec<Outcome> {
    let mut out = VecDeque::new();
    conn.ingest(buffer(bytes), &mut out);
    out.into_iter().collect()
  }

  /// Feed one receive and return its outcomes with the receive allocation's address range.
  fn feed_tracked(conn: &mut Connection, bytes: &[u8]) -> (Vec<Outcome>, Range<usize>) {
    let b = buffer(bytes);
    let start = b.as_init().as_ptr() as usize;
    let range = start..start + b.as_init().len();
    let mut out = VecDeque::new();
    conn.ingest(b, &mut out);
    (out.into_iter().collect(), range)
  }

  fn request(o: &Outcome) -> &Exchange {
    match o {
      Outcome::Request(x) => x,
      other => panic!("unexpected outcome {other:?}"),
    }
  }

  fn segment(o: &Outcome) -> &FrozenBuffer {
    match o {
      Outcome::Segment(s) => s,
      other => panic!("unexpected outcome {other:?}"),
    }
  }

  fn error(o: &Outcome) -> ParseError {
    match o {
      Outcome::Error(e) => *e,
      other => panic!("unexpected outcome {other:?}"),
    }
  }

  /// Body bytes across `out`, asserting the `Request → Segment* → BodyEnd` order.
  fn body_of(out: &[Outcome]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut ended = false;
    for (i, o) in out.iter().enumerate() {
      match o {
        Outcome::Request(_) => assert_eq!(i, 0),
        Outcome::Segment(s) => {
          assert!(!ended);
          body.extend_from_slice(s.as_ref());
        }
        Outcome::BodyEnd => ended = true,
        Outcome::Error(e) => panic!("unexpected error {e:?}"),
      }
    }
    assert!(ended, "{out:?}");
    body
  }

  const CHUNKED: &[u8] =
    b"POST /up HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";

  #[test]
  fn expect_continue_is_only_for_http11_bodies() {
    for (version, length, fields, expected) in [
      ("1.0", 1, "Expect: 100-continue", false),
      ("1.0", 1, "Expect: fancy", false),
      ("1.1", 0, "Expect: 100-continue", false),
      (
        "1.1",
        1,
        "Expect: 100-continue, 100-CONTINUE\r\nExpect: 100-continue",
        true,
      ),
    ] {
      let input = format!("POST / HTTP/{version}\r\nContent-Length: {length}\r\n{fields}\r\n\r\n");
      for split in 1..input.len() {
        let mut conn = Connection::new(budget());
        let mut out = feed(&mut conn, &input.as_bytes()[..split]);
        out.extend(feed(&mut conn, &input.as_bytes()[split..]));
        assert_eq!(out.len(), 1);
        assert_eq!(request(&out[0]).expect_continue, expected);
      }
    }
  }

  #[test]
  fn duplicate_content_lengths_must_agree() {
    for (second, valid) in [("1", true), ("2", false), ("x", false)] {
      let mut conn = Connection::new(budget());
      let input = format!("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: {second}\r\n\r\nx");
      let out = feed(&mut conn, input.as_bytes());
      assert_eq!(matches!(out[0], Outcome::Request(_)), valid, "{second}");
    }
  }

  #[test]
  fn close_wins_across_connection_fields() {
    for version in ["1.0", "1.1"] {
      for fields in [
        "Connection: close\r\nConnection: keep-alive",
        "Connection: keep-alive\r\nConnection: close",
      ] {
        let mut conn = Connection::new(budget());
        let out = feed(
          &mut conn,
          format!("GET / HTTP/{version}\r\n{fields}\r\n\r\n").as_bytes(),
        );
        assert!(!request(&out[0]).keep_alive);
      }
    }
  }

  #[test]
  fn header_limit_allows_exactly_the_limit() {
    for count in [MAX_HEADERS, MAX_HEADERS + 1] {
      let input = format!("GET / HTTP/1.1\r\n{}\r\n", "X: a\r\n".repeat(count));
      let mut conn = Connection::new(budget());
      let out = feed(&mut conn, input.as_bytes());
      assert_eq!(matches!(out[0], Outcome::Request(_)), count == MAX_HEADERS);
    }
  }

  #[test]
  fn parses_a_simple_get_zero_copy() {
    let mut conn = Connection::new(budget());
    let out = feed(
      &mut conn,
      b"GET /echo?x=1 HTTP/1.1\r\nHost: a\r\nUser-Agent: oha\r\n\r\n",
    );
    assert_eq!(out.len(), 1);
    let x = request(&out[0]);
    assert_eq!(x.method, Method::Get);
    assert_eq!(x.path_bytes(), b"/echo?x=1");
    assert_eq!(x.version, 1);
    assert_eq!(x.headers.len(), 2);
    assert_eq!(x.header_name(1).unwrap(), b"User-Agent");
    assert_eq!(x.header(b"host").unwrap(), b"a");
    assert!(x.keep_alive);
    assert!(!x.has_body);
    assert!(!conn.is_closed());
    assert!(!conn.body_pending());
  }

  #[test]
  fn splits_pipelined_requests_in_one_receive() {
    let mut conn = Connection::new(budget());
    let out = feed(
      &mut conn,
      b"GET /a HTTP/1.1\r\nHost: a\r\n\r\nGET /b HTTP/1.1\r\nHost: a\r\n\r\n",
    );
    assert_eq!(out.len(), 2);
    assert_eq!(request(&out[0]).path_bytes(), b"/a");
    assert_eq!(request(&out[1]).path_bytes(), b"/b");
  }

  #[test]
  fn assembles_a_head_split_across_receives() {
    let mut conn = Connection::new(budget());
    assert!(feed(&mut conn, b"GET /split HTTP/1.1\r\nHo").is_empty());
    let out = feed(&mut conn, b"st: a\r\nX-A: 1\r\n\r\nGET /next HTTP/1.1\r\n\r\n");
    assert_eq!(out.len(), 2);
    let x = request(&out[0]);
    assert_eq!(x.path_bytes(), b"/split");
    assert_eq!(x.header(b"x-a").unwrap(), b"1");
    assert_eq!(request(&out[1]).path_bytes(), b"/next");
  }

  #[test]
  fn buffers_a_content_length_body_across_receives() {
    let mut conn = Connection::new(budget());
    let (out, first) = feed_tracked(&mut conn, b"POST /echo HTTP/1.1\r\nContent-Length: 11\r\n\r\nhello");
    assert_eq!(out.len(), 2);
    let x = request(&out[0]);
    assert_eq!(x.method, Method::Post);
    assert!(x.has_body);
    let s = segment(&out[1]);
    assert_eq!(s.as_ref(), b"hello");
    assert!(first.contains(&(s.as_ref().as_ptr() as usize)));
    assert!(conn.body_pending());
    let (out, second) = feed_tracked(&mut conn, b" worldGET /after HTTP/1.1\r\n\r\n");
    assert_eq!(out.len(), 3);
    let s = segment(&out[0]);
    assert_eq!(s.as_ref(), b" world");
    assert!(second.contains(&(s.as_ref().as_ptr() as usize)));
    assert!(matches!(out[1], Outcome::BodyEnd));
    assert_eq!(request(&out[2]).path_bytes(), b"/after");
    assert!(!conn.body_pending());
  }

  #[test]
  fn chunked_body_in_one_receive_is_zero_copy() {
    let mut conn = Connection::new(budget());
    let (out, range) = feed_tracked(&mut conn, CHUNKED);
    assert_eq!(out.len(), 4, "{out:?}");
    assert!(request(&out[0]).has_body);
    for (o, expected) in out[1..3].iter().zip([&b"hello"[..], b" world"]) {
      let s = segment(o);
      assert_eq!(s.as_ref(), expected);
      let ptr = s.as_ref().as_ptr() as usize;
      assert!(range.contains(&ptr) && range.contains(&(ptr + s.as_ref().len() - 1)));
    }
    assert!(matches!(out[3], Outcome::BodyEnd));
    assert!(!conn.body_pending());
    assert!(!conn.is_closed());
  }

  #[test]
  fn chunked_body_split_at_every_offset() {
    let mut conn = Connection::new(budget());
    let whole = body_of(&feed(&mut conn, CHUNKED));
    for split in 1..CHUNKED.len() {
      let mut conn = Connection::new(budget());
      let mut out = feed(&mut conn, &CHUNKED[..split]);
      out.extend(feed(&mut conn, &CHUNKED[split..]));
      assert_eq!(body_of(&out), whole, "split at {split}");
      assert!(!conn.body_pending(), "split at {split}");
    }
  }

  #[test]
  fn chunk_extensions_are_ignored() {
    let wire =
      b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5;a=b\r\nhello\r\n1 ; x=\"y\"\r\n!\r\n0;end\r\n\r\n";
    let mut conn = Connection::new(budget());
    assert_eq!(body_of(&feed(&mut conn, wire)), b"hello!");
    for split in 1..wire.len() {
      let mut conn = Connection::new(budget());
      let mut out = feed(&mut conn, &wire[..split]);
      out.extend(feed(&mut conn, &wire[split..]));
      assert_eq!(body_of(&out), b"hello!", "split at {split}");
    }
  }

  #[test]
  fn trailers_are_dropped_and_bounded() {
    let wire = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n0\r\nX-Sum: 1\r\nX-Other: 2\r\n\r\nGET /next HTTP/1.1\r\n\r\n";
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, wire);
    assert_eq!(out.len(), 4, "{out:?}");
    assert_eq!(segment(&out[1]).as_ref(), b"x");
    assert!(matches!(out[2], Outcome::BodyEnd));
    assert_eq!(request(&out[3]).path_bytes(), b"/next");
    for split in 1..wire.len() {
      let mut conn = Connection::new(budget());
      let mut out = feed(&mut conn, &wire[..split]);
      out.extend(feed(&mut conn, &wire[split..]));
      assert_eq!(out.len(), 4, "split at {split}: {out:?}");
      assert_eq!(request(&out[3]).path_bytes(), b"/next");
    }
    let big = format!(
      "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX: {}\r\n\r\n",
      "a".repeat(MAX_HEAD_BYTES)
    );
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, big.as_bytes());
    assert_eq!(error(&out[1]), ParseError::HeadTooLarge);
    let mut conn = Connection::new(budget());
    let mut out = feed(&mut conn, &big.as_bytes()[..60]);
    out.extend(feed(&mut conn, &big.as_bytes()[60..]));
    assert_eq!(error(out.last().unwrap()), ParseError::HeadTooLarge);
    assert!(conn.is_closed());
  }

  #[test]
  fn malformed_chunk_lines_are_bad_requests() {
    for tail in [
      &b"zz\r\nhello\r\n0\r\n\r\n"[..],
      b"11111111111111111\r\n",
      b"5\nhello\r\n0\r\n\r\n",
      b" 5\r\nhello\r\n0\r\n\r\n",
      b"5\r\nhello\r\r0\r\n\r\n",
      b"5\r\nhelloXX0\r\n\r\n",
      b"5;ext\nhello\r\n0\r\n\r\n",
    ] {
      let mut wire = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
      wire.extend_from_slice(tail);
      let mut conn = Connection::new(budget());
      let out = feed(&mut conn, &wire);
      assert_eq!(
        error(out.last().unwrap()),
        ParseError::BadRequest,
        "{}",
        String::from_utf8_lossy(tail)
      );
      assert!(conn.is_closed());
    }
    let mut conn = Connection::new(budget());
    let long = format!(
      "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1;{}\r\nx\r\n0\r\n\r\n",
      "e".repeat(MAX_CHUNK_LINE_BYTES)
    );
    let out = feed(&mut conn, long.as_bytes());
    assert_eq!(error(out.last().unwrap()), ParseError::BadRequest);
  }

  #[test]
  fn transfer_encoding_header_validation() {
    for (headers, expected) in [
      (
        "Transfer-Encoding: chunked\r\nContent-Length: 5",
        ParseError::BadRequest,
      ),
      (
        "Transfer-Encoding: gzip\r\nTransfer-Encoding: chunked",
        ParseError::UnsupportedTransferEncoding,
      ),
      (
        "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked",
        ParseError::UnsupportedTransferEncoding,
      ),
      ("Transfer-Encoding: gzip", ParseError::UnsupportedTransferEncoding),
      (
        "Transfer-Encoding: chunked, gzip",
        ParseError::UnsupportedTransferEncoding,
      ),
      (
        "Transfer-Encoding: gzip, chunked",
        ParseError::UnsupportedTransferEncoding,
      ),
      (
        "Transfer-Encoding: chunked, chunked",
        ParseError::UnsupportedTransferEncoding,
      ),
    ] {
      let wire = format!("POST / HTTP/1.1\r\n{headers}\r\n\r\n");
      for split in 1..=wire.len() {
        let mut conn = Connection::new(budget());
        let mut out = feed(&mut conn, &wire.as_bytes()[..split]);
        out.extend(feed(&mut conn, &wire.as_bytes()[split..]));
        assert_eq!(error(&out[0]), expected, "{headers}, split {split}");
      }
    }
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n");
    assert_eq!(error(&out[0]), ParseError::BadRequest);
    let mut conn = Connection::new(budget());
    let out = feed(
      &mut conn,
      b"POST / HTTP/1.1\r\nTransfer-Encoding: Chunked\r\n\r\n0\r\n\r\n",
    );
    assert!(request(&out[0]).has_body);
    assert!(matches!(out[1], Outcome::BodyEnd));
  }

  #[test]
  fn pipelined_request_follows_a_chunked_body() {
    let mut wire = CHUNKED.to_vec();
    wire.extend_from_slice(b"GET /after HTTP/1.1\r\n\r\n");
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, &wire);
    assert_eq!(out.len(), 5, "{out:?}");
    assert!(matches!(out[3], Outcome::BodyEnd));
    assert_eq!(request(&out[4]).path_bytes(), b"/after");
    let mut conn = Connection::new(budget());
    let mut out = feed(&mut conn, &wire[..CHUNKED.len() - 3]);
    out.extend(feed(&mut conn, &wire[CHUNKED.len() - 3..]));
    assert_eq!(request(out.last().unwrap()).path_bytes(), b"/after");
  }

  #[test]
  fn discarded_body_emits_nothing_and_keeps_framing() {
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"POST / HTTP/1.1\r\nContent-Length: 11\r\n\r\nhello");
    assert_eq!(out.len(), 2);
    conn.discard_body();
    let out = feed(&mut conn, b" worldGET /after HTTP/1.1\r\n\r\n");
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(request(&out[0]).path_bytes(), b"/after");
  }

  #[test]
  fn body_of_a_closing_request_still_flows() {
    let mut conn = Connection::new(budget());
    let out = feed(
      &mut conn,
      b"POST / HTTP/1.1\r\nConnection: close\r\nContent-Length: 4\r\n\r\nab",
    );
    assert!(conn.is_closed());
    assert_eq!(out.len(), 2);
    let out = feed(&mut conn, b"cdGET / HTTP/1.1\r\n\r\n");
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(segment(&out[0]).as_ref(), b"cd");
    assert!(matches!(out[1], Outcome::BodyEnd));
  }

  #[test]
  fn connection_close_and_http10_semantics() {
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
    assert!(!request(&out[0]).keep_alive);
    assert!(conn.is_closed());
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"GET / HTTP/1.0\r\n\r\n");
    assert!(!request(&out[0]).keep_alive);
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"GET / HTTP/1.0\r\nConnection: Keep-Alive\r\n\r\n");
    assert!(request(&out[0]).keep_alive);
  }

  #[test]
  fn methods_map_to_their_codes() {
    for (name, method) in [
      ("GET", Method::Get),
      ("HEAD", Method::Head),
      ("POST", Method::Post),
      ("PUT", Method::Put),
      ("DELETE", Method::Delete),
      ("CONNECT", Method::Connect),
      ("OPTIONS", Method::Options),
      ("TRACE", Method::Trace),
      ("PATCH", Method::Patch),
      ("PROPFIND", Method::Other),
      ("get", Method::Other),
    ] {
      let mut conn = Connection::new(budget());
      let out = feed(&mut conn, format!("{name} / HTTP/1.1\r\n\r\n").as_bytes());
      let x = request(&out[0]);
      assert_eq!(x.method, method, "{name}");
      assert_eq!(x.method_bytes(), name.as_bytes());
    }
  }

  #[test]
  fn parse_errors_map_to_response_statuses() {
    assert_eq!(ParseError::BadRequest.status(), 400);
    assert_eq!(ParseError::HeadTooLarge.status(), 431);
    assert_eq!(ParseError::UnsupportedTransferEncoding.status(), 501);
    assert_eq!(ParseError::UnsupportedExpectation.status(), 417);
    assert_eq!(ParseError::OutOfMemory.status(), 503);
  }

  #[test]
  fn unterminated_oversized_heads_are_too_large() {
    let line = format!("GET /{}", "a".repeat(MAX_HEAD_BYTES + 8));
    let mut conn = Connection::new(budget());
    assert_eq!(error(&feed(&mut conn, line.as_bytes())[0]), ParseError::HeadTooLarge);
    let fields = format!("GET / HTTP/1.1\r\nX: {}", "a".repeat(MAX_HEAD_BYTES + 8));
    let mut conn = Connection::new(budget());
    assert_eq!(error(&feed(&mut conn, fields.as_bytes())[0]), ParseError::HeadTooLarge);
    assert!(conn.is_closed());
    assert!(
      feed(&mut conn, b"GET / HTTP/1.1\r\n\r\n").is_empty(),
      "closed connections ignore input"
    );
  }

  #[test]
  fn split_heads_fail_on_overflow_bad_bytes_and_exhausted_budget() {
    let mut conn = Connection::new(budget());
    assert!(feed(&mut conn, b"GET / HTTP/1.1\r\nX: a").is_empty());
    let out = feed(&mut conn, "a".repeat(MAX_HEAD_BYTES).as_bytes());
    assert_eq!(error(&out[0]), ParseError::HeadTooLarge);
    assert!(conn.is_closed());

    let mut conn = Connection::new(budget());
    assert!(feed(&mut conn, b"GET / HTTP/1.1\r\n").is_empty());
    let out = feed(&mut conn, b"Bad Header: x\r\n\r\n");
    assert_eq!(error(&out[0]), ParseError::BadRequest);

    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"GET / HTTP/1.1\r\nBad Header: x\r\n\r\n");
    assert_eq!(error(&out[0]), ParseError::BadRequest);

    // A split head is copied into owner storage, which the connection's budget must admit.
    let mut conn = Connection::new(Budget::new(8));
    assert!(feed(&mut conn, b"GET / HTTP/1.1\r\nHost:").is_empty());
    let out = feed(&mut conn, b" a\r\n\r\n");
    assert_eq!(error(&out[0]), ParseError::OutOfMemory);
    assert!(conn.is_closed());
  }

  #[test]
  fn trailers_may_span_three_receives() {
    let mut conn = Connection::new(budget());
    let mut out = feed(
      &mut conn,
      b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-A: 1",
    );
    out.extend(feed(&mut conn, b"\r\nX-B: 2"));
    assert!(conn.body_pending());
    out.extend(feed(&mut conn, b"\r\n\r\nGET /next HTTP/1.1\r\n\r\n"));
    assert_eq!(out.len(), 3, "{out:?}");
    assert!(matches!(out[1], Outcome::BodyEnd));
    assert_eq!(request(&out[2]).path_bytes(), b"/next");
    assert!(!conn.body_pending());
  }

  #[test]
  fn empty_list_members_are_ignored_in_expect_and_transfer_encoding() {
    let mut conn = Connection::new(budget());
    let out = feed(
      &mut conn,
      b"POST / HTTP/1.1\r\nExpect: , 100-continue,\r\nContent-Length: 1\r\n\r\nx",
    );
    assert!(request(&out[0]).expect_continue);
    let mut conn = Connection::new(budget());
    let out = feed(
      &mut conn,
      b"POST / HTTP/1.1\r\nTransfer-Encoding: , chunked\r\n\r\n1\r\nx\r\n0\r\n\r\n",
    );
    assert_eq!(body_of(&out), b"x");
    let mut conn = Connection::new(budget());
    let out = feed(
      &mut conn,
      b"POST / HTTP/1.1\r\nExpect: 100-continue, fancy\r\nContent-Length: 1\r\n\r\nx",
    );
    assert_eq!(error(&out[0]), ParseError::UnsupportedExpectation);
  }

  #[test]
  fn content_length_must_be_a_bounded_decimal() {
    for value in ["", "12345678901234567890", "-1", "+1", "1 2", "0x10"] {
      let mut conn = Connection::new(budget());
      let out = feed(
        &mut conn,
        format!("POST / HTTP/1.1\r\nContent-Length: {value}\r\n\r\n").as_bytes(),
      );
      assert_eq!(error(&out[0]), ParseError::BadRequest, "{value:?}");
    }
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"POST / HTTP/1.1\r\nContent-Length:  0 \r\n\r\n");
    let x = request(&out[0]);
    assert_eq!(x.content_length, Some(0));
    assert!(!x.has_body);
  }

  #[test]
  fn bounded_ingest_returns_unparsed_heads() {
    let mut conn = Connection::new(budget());
    let mut out = VecDeque::new();
    let wire = buffer(b"GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n").freeze();
    let rest = conn.ingest_bounded(wire, 1, &mut out).expect("second head is returned");
    assert_eq!(out.len(), 1);
    assert_eq!(request(&out[0]).path_bytes(), b"/a");
    assert_eq!(rest.as_ref(), b"GET /b HTTP/1.1\r\n\r\n");
    out.clear();
    assert!(conn.ingest_bounded(rest, 1, &mut out).is_none());
    assert_eq!(request(&out[0]).path_bytes(), b"/b");
    // Body bytes flow without head credit.
    let mut conn = Connection::new(budget());
    let mut out = VecDeque::new();
    let wire = buffer(b"POST / HTTP/1.1\r\nContent-Length: 2\r\n\r\nab").freeze();
    assert!(conn.ingest_bounded(wire, 1, &mut out).is_none());
    assert_eq!(body_of(&out.into_iter().collect::<Vec<_>>()), b"ab");
  }

  #[test]
  fn rejects_malformed_and_oversized_input() {
    let mut conn = Connection::new(budget());
    let out = feed(&mut conn, b"GARBAGE\x01\r\n\r\n");
    assert!(matches!(out[0], Outcome::Error(ParseError::BadRequest)));
    assert!(conn.is_closed());
    let mut conn = Connection::new(budget());
    let big = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD_BYTES));
    let out = feed(&mut conn, big.as_bytes());
    assert!(matches!(out[0], Outcome::Error(ParseError::HeadTooLarge)));
  }
}
