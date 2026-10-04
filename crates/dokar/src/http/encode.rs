/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! HTTP/1.x response encoding into native send storage.

use std::io;
use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use compio_buf::{IoBufMut, SetLen};

use crate::buffer::{Budget, Buffer, FrozenBuffer};

/// One response header as borrowed bytes.
#[derive(Clone, Copy, Debug)]
pub struct ResponseHeader<'a> {
  pub name: &'a [u8],
  pub value: &'a [u8],
}

const DATE_LEN: usize = 37;
const SERVER: &[u8] = b"server: elide\r\n";

/// `date: <IMF-fixdate>\r\n`, reformatted at most once per second.
#[derive(Debug)]
pub struct DateCache {
  second: u64,
  line: [u8; DATE_LEN],
}

impl Default for DateCache {
  fn default() -> Self {
    Self {
      second: u64::MAX,
      line: [0; DATE_LEN],
    }
  }
}

impl DateCache {
  pub fn line(&mut self) -> &[u8] {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if now != self.second {
      self.second = now;
      format_date(now, &mut self.line);
    }
    &self.line
  }
}

/// Writes `date: Wed, 16 Sep 2026 21:48:39 GMT\r\n` for `secs` since the Unix epoch.
fn format_date(secs: u64, out: &mut [u8; DATE_LEN]) {
  const DAYS: [&[u8; 3]; 7] = [b"Thu", b"Fri", b"Sat", b"Sun", b"Mon", b"Tue", b"Wed"];
  const MONTHS: [&[u8; 3]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
  ];
  let days = secs / 86_400;
  let rem = secs % 86_400;
  let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
  // Civil-from-days (Howard Hinnant), valid for the years we can reach.
  let z = days as i64 + 719_468;
  let era = z.div_euclid(146_097);
  let doe = z.rem_euclid(146_097);
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let mut y = yoe + era * 400;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let d = doy - (153 * mp + 2) / 5 + 1;
  let mon = if mp < 10 { mp + 3 } else { mp - 9 };
  if mon <= 2 {
    y += 1;
  }
  let mut w = *b"date: Ddd, 00 Mmm 0000 00:00:00 GMT\r\n";
  w[6..9].copy_from_slice(DAYS[(days % 7) as usize]);
  two(&mut w[11..13], d as u64);
  w[14..17].copy_from_slice(MONTHS[(mon - 1) as usize]);
  let y = y as u64;
  two(&mut w[18..20], y / 100);
  two(&mut w[20..22], y % 100);
  two(&mut w[23..25], h);
  two(&mut w[26..28], m);
  two(&mut w[29..31], s);
  *out = w;
}

fn two(out: &mut [u8], v: u64) {
  out[0] = b'0' + (v / 10 % 10) as u8;
  out[1] = b'0' + (v % 10) as u8;
}

fn reason(status: u16) -> &'static [u8] {
  match status {
    200 => b"OK",
    201 => b"Created",
    202 => b"Accepted",
    204 => b"No Content",
    206 => b"Partial Content",
    301 => b"Moved Permanently",
    302 => b"Found",
    303 => b"See Other",
    304 => b"Not Modified",
    307 => b"Temporary Redirect",
    308 => b"Permanent Redirect",
    400 => b"Bad Request",
    401 => b"Unauthorized",
    403 => b"Forbidden",
    404 => b"Not Found",
    405 => b"Method Not Allowed",
    408 => b"Request Timeout",
    409 => b"Conflict",
    411 => b"Length Required",
    413 => b"Payload Too Large",
    415 => b"Unsupported Media Type",
    422 => b"Unprocessable Entity",
    429 => b"Too Many Requests",
    431 => b"Request Header Fields Too Large",
    500 => b"Internal Server Error",
    501 => b"Not Implemented",
    502 => b"Bad Gateway",
    503 => b"Service Unavailable",
    504 => b"Gateway Timeout",
    _ => b"Unknown",
  }
}

/// Headers the encoder owns; caller-supplied copies are dropped.
fn owned(name: &[u8]) -> bool {
  name.eq_ignore_ascii_case(b"content-length")
    || name.eq_ignore_ascii_case(b"connection")
    || name.eq_ignore_ascii_case(b"date")
    || name.eq_ignore_ascii_case(b"server")
    || name.eq_ignore_ascii_case(b"transfer-encoding")
    || name.eq_ignore_ascii_case(b"trailer")
}

/// How the response body is delimited on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
  /// `content-length: n`.
  Length(u64),
  /// `transfer-encoding: chunked`; the caller frames each part with [`frame_chunk`].
  Chunked,
  /// No framing header: the body ends when the connection closes.
  Close,
}

/// Bytes reserved before a chunk payload for its size line (16 hex digits and CRLF).
pub const CHUNK_HEADROOM: usize = 18;
/// Bytes reserved after a chunk payload for its CRLF and the `0\r\n\r\n` terminator.
pub const CHUNK_TAIL: usize = 7;

/// Frame a payload already written at `CHUNK_HEADROOM..CHUNK_HEADROOM + payload_len` in
/// place; returns the range to send. `buffer` must have room for `CHUNK_TAIL` after the
/// payload. An empty final part yields the terminator alone.
///
/// # Safety
/// The payload range must be initialized; it may have been written through a foreign pointer.
pub unsafe fn frame_chunk(buffer: &mut Buffer, payload_len: usize, final_part: bool) -> Range<usize> {
  const TERMINATOR: &[u8] = b"0\r\n\r\n";
  let end = CHUNK_HEADROOM
    .checked_add(payload_len)
    .expect("chunk payload length overflow");
  let raw = buffer.as_uninit();
  if payload_len == 0 && final_part {
    for (slot, byte) in raw[..end].iter_mut().zip(std::iter::repeat(0u8)) {
      slot.write(byte);
    }
    for (slot, byte) in raw[end..end + TERMINATOR.len()].iter_mut().zip(TERMINATOR) {
      slot.write(*byte);
    }
    // SAFETY: Both the headroom and terminator were initialized above within the checked slices.
    unsafe { buffer.set_len(end + TERMINATOR.len()) };
    return end..end + TERMINATOR.len();
  }
  let mut start = CHUNK_HEADROOM - 2;
  let mut n = payload_len;
  loop {
    start -= 1;
    raw[start].write(b"0123456789abcdef"[n & 0xf]);
    n >>= 4;
    if n == 0 {
      break;
    }
  }
  for slot in &mut raw[..start] {
    slot.write(0);
  }
  raw[CHUNK_HEADROOM - 2].write(b'\r');
  raw[CHUNK_HEADROOM - 1].write(b'\n');
  raw[end].write(b'\r');
  raw[end + 1].write(b'\n');
  let mut stop = end + 2;
  if final_part {
    for (slot, byte) in raw[stop..stop + TERMINATOR.len()].iter_mut().zip(TERMINATOR) {
      slot.write(*byte);
    }
    stop += TERMINATOR.len();
  }
  // SAFETY: The payload prefix was initialized on entry; framing and tail writes initialized the remainder.
  unsafe { buffer.set_len(stop) };
  start..stop
}

/// Write the status line and headers (through the blank line) into `head`.
#[allow(clippy::too_many_arguments)]
fn write_head(
  head: &mut Vec<u8>,
  date: &mut DateCache,
  version: u8,
  status: u16,
  headers: &[ResponseHeader<'_>],
  framing: Framing,
  head_only: bool,
  keep_alive: bool,
) -> io::Result<()> {
  for header in headers {
    if header.name.is_empty()
      || !header
        .name
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
      || !header.value.iter().all(|b| *b == b'\t' || (*b >= b' ' && *b != 0x7f))
    {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid HTTP response header",
      ));
    }
  }
  head.extend_from_slice(if version == 0 { b"HTTP/1.0 " } else { b"HTTP/1.1 " });
  let mut code = [0u8; 3];
  code[0] = b'0' + (status / 100 % 10) as u8;
  code[1] = b'0' + (status / 10 % 10) as u8;
  code[2] = b'0' + (status % 10) as u8;
  head.extend_from_slice(&code);
  head.push(b' ');
  head.extend_from_slice(reason(status));
  head.extend_from_slice(b"\r\n");
  for header in headers {
    if owned(header.name) {
      continue;
    }
    head.extend_from_slice(header.name);
    head.extend_from_slice(b": ");
    head.extend_from_slice(header.value);
    head.extend_from_slice(b"\r\n");
  }
  let no_length = status == 204 || status == 304 || (100..200).contains(&status);
  // 205 still needs message framing (RFC 9112 §6.3) and its body is always empty.
  // HTTP/1.0 cannot carry chunked; an unknown length there ends with the connection.
  let framing = match framing {
    Framing::Chunked if status == 205 => Framing::Length(0),
    Framing::Chunked if version == 0 => Framing::Close,
    other => other,
  };
  let keep_alive = keep_alive && framing != Framing::Close;
  match framing {
    Framing::Length(length) if !no_length => {
      head.extend_from_slice(b"content-length: ");
      let mut digits = [0u8; 20];
      let mut n = if status == 205 { 0 } else { length };
      let mut i = digits.len();
      loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
          break;
        }
      }
      head.extend_from_slice(&digits[i..]);
      head.extend_from_slice(b"\r\n");
    }
    Framing::Chunked if !(no_length || head_only) => {
      head.extend_from_slice(b"transfer-encoding: chunked\r\n");
    }
    _ => {}
  }
  match (version, keep_alive) {
    (0, true) => head.extend_from_slice(b"connection: keep-alive\r\n"),
    (_, false) if version != 0 => head.extend_from_slice(b"connection: close\r\n"),
    _ => {}
  }
  head.extend_from_slice(SERVER);
  head.extend_from_slice(date.line());
  head.extend_from_slice(b"\r\n");
  Ok(())
}

/// Encode a response head (status line through the blank line) into frozen native storage.
///
/// `Framing::Close`, and `Framing::Chunked` on HTTP/1.0, force `keep_alive` off. HEAD
/// responses and bodiless statuses never carry `transfer-encoding`; HEAD with
/// `Framing::Length` still advertises the length.
///
/// # Errors
/// Returns an error for invalid headers or when response storage cannot be reserved.
#[allow(clippy::too_many_arguments)]
pub fn encode_head(
  budget: &Budget,
  date: &mut DateCache,
  version: u8,
  status: u16,
  headers: &[ResponseHeader<'_>],
  framing: Framing,
  head_only: bool,
  keep_alive: bool,
) -> io::Result<FrozenBuffer> {
  let mut head: Vec<u8> = Vec::with_capacity(160 + headers.len() * 32);
  write_head(
    &mut head, date, version, status, headers, framing, head_only, keep_alive,
  )?;
  let mut storage = Buffer::new(head.len(), budget.clone())?;
  storage.write(0, &head)?;
  Ok(storage.freeze())
}

/// Encode one complete response into frozen native storage.
///
/// `body` is omitted from the wire when `head_only` (HEAD requests) but still
/// sizes `content-length`. A `connection` header is written only when it
/// deviates from the version default.
///
/// # Errors
/// Returns an error for invalid headers or when response storage cannot be reserved.
#[allow(clippy::too_many_arguments)]
pub fn encode_response(
  budget: &Budget,
  date: &mut DateCache,
  version: u8,
  status: u16,
  headers: &[ResponseHeader<'_>],
  body: &[u8],
  head_only: bool,
  keep_alive: bool,
) -> io::Result<FrozenBuffer> {
  let mut head: Vec<u8> = Vec::with_capacity(160 + headers.len() * 32);
  write_head(
    &mut head,
    date,
    version,
    status,
    headers,
    Framing::Length(body.len() as u64),
    head_only,
    keep_alive,
  )?;
  let no_body = status == 204 || status == 205 || status == 304 || (100..200).contains(&status);
  let body_len = if head_only || no_body { 0 } else { body.len() };
  let mut storage = Buffer::new(head.len() + body_len, budget.clone())?;
  storage.write(0, &head)?;
  if body_len > 0 {
    storage.write(head.len(), body)?;
  }
  Ok(storage.freeze())
}

#[cfg(test)]
mod tests {
  use compio_buf::IoBuf;

  use super::*;

  fn encode(
    status: u16,
    headers: &[ResponseHeader<'_>],
    body: &[u8],
    head_only: bool,
    keep_alive: bool,
    version: u8,
  ) -> String {
    let budget = Budget::new(1 << 20);
    let mut date = DateCache::default();
    let frozen = encode_response(
      &budget, &mut date, version, status, headers, body, head_only, keep_alive,
    )
    .unwrap();
    String::from_utf8(frozen.as_ref().to_vec()).unwrap()
  }

  fn strip_date(s: &str) -> String {
    s.split("\r\n")
      .filter(|l| !l.starts_with("date: "))
      .collect::<Vec<_>>()
      .join("\r\n")
  }

  #[test]
  fn no_body_statuses_never_emit_payload() {
    for status in [100, 101, 199, 204, 205, 304] {
      let wire = encode(status, &[], b"forbidden", false, true, 1);
      assert!(wire.ends_with("\r\n\r\n"), "{status}: {wire}");
      if status == 205 {
        assert!(wire.contains("content-length: 0\r\n"));
      }
    }
  }

  #[test]
  fn rejects_invalid_header_bytes() {
    for (name, value) in [
      (&b"x-test"[..], &b"ok\r\nx-injected: yes"[..]),
      (b"bad:name", b"ok"),
      (b"", b"ok"),
      (b"bad name", b"ok"),
      (b"x-test", b"bad\0value"),
      (b"x-test", b"bad\x7fvalue"),
    ] {
      assert!(
        encode_response(
          &Budget::new(1 << 20),
          &mut DateCache::default(),
          1,
          200,
          &[ResponseHeader { name, value }],
          b"",
          false,
          true
        )
        .is_err()
      );
    }
  }

  #[test]
  fn encodes_ok_with_body_and_passthrough_headers() {
    let out = encode(
      200,
      &[ResponseHeader {
        name: b"content-type",
        value: b"text/plain",
      }],
      b"hello",
      false,
      true,
      1,
    );
    assert!(out.contains("date: "), "{out}");
    assert_eq!(
      strip_date(&out),
      "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\nserver: elide\r\n\r\nhello"
    );
  }

  #[test]
  fn head_only_sizes_but_omits_body() {
    let out = encode(200, &[], b"hello", true, true, 1);
    assert!(out.contains("content-length: 5\r\n"));
    assert!(out.ends_with("\r\n\r\n"), "{out}");
  }

  #[test]
  fn connection_header_follows_version_defaults() {
    assert!(encode(404, &[], b"", false, false, 1).contains("connection: close\r\n"));
    assert!(!encode(404, &[], b"", false, true, 1).contains("connection:"));
    assert!(encode(200, &[], b"", false, true, 0).contains("connection: keep-alive\r\n"));
    assert!(!encode(200, &[], b"", false, false, 0).contains("connection:"));
  }

  #[test]
  fn drops_caller_framing_headers_and_skips_length_for_204() {
    let out = encode(
      204,
      &[
        ResponseHeader {
          name: b"Content-Length",
          value: b"99",
        },
        ResponseHeader {
          name: b"X-Keep",
          value: b"1",
        },
      ],
      b"",
      false,
      true,
      1,
    );
    assert!(!out.contains("content-length"), "{out}");
    assert!(out.contains("X-Keep: 1\r\n"));
  }

  #[test]
  fn formats_imf_fixdate() {
    let mut line = [0u8; DATE_LEN];
    format_date(1_789_594_119, &mut line); // 2026-09-16T21:28:39Z
    assert_eq!(
      std::str::from_utf8(&line).unwrap(),
      "date: Wed, 16 Sep 2026 21:28:39 GMT\r\n"
    );
    format_date(0, &mut line);
    assert_eq!(
      std::str::from_utf8(&line).unwrap(),
      "date: Thu, 01 Jan 1970 00:00:00 GMT\r\n"
    );
  }

  fn head(
    version: u8,
    status: u16,
    headers: &[ResponseHeader<'_>],
    framing: Framing,
    head_only: bool,
    keep_alive: bool,
  ) -> String {
    let frozen = encode_head(
      &Budget::new(1 << 20),
      &mut DateCache::default(),
      version,
      status,
      headers,
      framing,
      head_only,
      keep_alive,
    )
    .unwrap();
    String::from_utf8(frozen.as_ref().to_vec()).unwrap()
  }

  #[test]
  fn encode_head_frames_by_kind() {
    let chunked = head(1, 200, &[], Framing::Chunked, false, true);
    assert!(chunked.contains("transfer-encoding: chunked\r\n"), "{chunked}");
    assert!(!chunked.contains("content-length"));
    assert!(!chunked.contains("connection:"));
    assert!(chunked.ends_with("\r\n\r\n"));
    let sized = head(1, 200, &[], Framing::Length(42), false, true);
    assert!(sized.contains("content-length: 42\r\n"), "{sized}");
    assert!(!sized.contains("transfer-encoding"));
    let close = head(1, 200, &[], Framing::Close, false, true);
    assert!(!close.contains("content-length") && !close.contains("transfer-encoding"));
    assert!(close.contains("connection: close\r\n"), "{close}");
    let legacy = head(0, 200, &[], Framing::Chunked, false, true);
    assert!(!legacy.contains("transfer-encoding"), "{legacy}");
    assert!(!legacy.contains("connection: keep-alive"), "{legacy}");
    assert_eq!(
      strip_date(&sized),
      "HTTP/1.1 200 OK\r\ncontent-length: 42\r\nserver: elide\r\n\r\n"
    );
  }

  #[test]
  fn encode_head_suppresses_transfer_encoding_without_a_body() {
    for (status, head_only) in [(200, true), (204, false), (304, false), (205, false)] {
      let out = head(1, status, &[], Framing::Chunked, head_only, true);
      assert!(!out.contains("transfer-encoding"), "{status}: {out}");
    }
    let out = head(1, 200, &[], Framing::Length(9), true, true);
    assert!(out.contains("content-length: 9\r\n"), "{out}");
  }

  #[test]
  fn encode_head_drops_caller_framing_headers() {
    let out = head(
      1,
      200,
      &[
        ResponseHeader {
          name: b"Trailer",
          value: b"x-sum",
        },
        ResponseHeader {
          name: b"Transfer-Encoding",
          value: b"gzip",
        },
        ResponseHeader {
          name: b"Content-Length",
          value: b"7",
        },
        ResponseHeader {
          name: b"X-Keep",
          value: b"1",
        },
      ],
      Framing::Chunked,
      false,
      true,
    );
    assert!(!out.contains("Trailer") && !out.contains("gzip") && !out.contains("Content-Length"));
    assert!(out.contains("transfer-encoding: chunked\r\n"));
    assert!(out.contains("X-Keep: 1\r\n"));
  }

  #[test]
  fn frame_chunk_writes_size_and_delimiters_in_place() {
    let payload = [b'x'; 0x1ab];
    let mut buffer = Buffer::new(CHUNK_HEADROOM + payload.len() + CHUNK_TAIL, Budget::new(1 << 20)).unwrap();
    buffer.write(0, &[0; CHUNK_HEADROOM]).unwrap();
    buffer.write(CHUNK_HEADROOM, &payload).unwrap();
    // SAFETY: This test initialized the payload in the owned buffer before framing it.
    let range = unsafe { frame_chunk(&mut buffer, payload.len(), false) };
    let wire = &buffer.as_init()[range.clone()];
    assert_eq!(range.start, CHUNK_HEADROOM - 2 - 3);
    assert!(wire.starts_with(b"1ab\r\n"));
    assert_eq!(&wire[5..5 + payload.len()], &payload[..]);
    assert!(wire.ends_with(b"x\r\n"));
    assert_eq!(wire.len(), 5 + payload.len() + 2);
    // SAFETY: This test initialized the payload in the owned buffer before framing it.
    let range = unsafe { frame_chunk(&mut buffer, payload.len(), true) };
    let wire = &buffer.as_init()[range];
    assert!(wire.ends_with(b"x\r\n0\r\n\r\n"));
    assert_eq!(wire.len(), 5 + payload.len() + CHUNK_TAIL);
  }

  #[test]
  fn status_lines_carry_the_standard_reason_phrase() {
    let cases: &[(u16, &str)] = &[
      (200, "OK"),
      (201, "Created"),
      (202, "Accepted"),
      (204, "No Content"),
      (206, "Partial Content"),
      (301, "Moved Permanently"),
      (302, "Found"),
      (303, "See Other"),
      (304, "Not Modified"),
      (307, "Temporary Redirect"),
      (308, "Permanent Redirect"),
      (400, "Bad Request"),
      (401, "Unauthorized"),
      (403, "Forbidden"),
      (404, "Not Found"),
      (405, "Method Not Allowed"),
      (408, "Request Timeout"),
      (409, "Conflict"),
      (411, "Length Required"),
      (413, "Payload Too Large"),
      (415, "Unsupported Media Type"),
      (422, "Unprocessable Entity"),
      (429, "Too Many Requests"),
      (431, "Request Header Fields Too Large"),
      (500, "Internal Server Error"),
      (501, "Not Implemented"),
      (502, "Bad Gateway"),
      (503, "Service Unavailable"),
      (504, "Gateway Timeout"),
      (599, "Unknown"),
    ];
    for (status, phrase) in cases {
      let out = head(1, *status, &[], Framing::Length(0), false, true);
      let line = format!("HTTP/1.1 {status} {phrase}\r\n");
      assert!(out.starts_with(&line), "{status}: {out}");
    }
  }

  #[test]
  fn encode_head_rejects_invalid_headers() {
    for header in [
      ResponseHeader { name: b"", value: b"v" },
      ResponseHeader {
        name: b"bad name",
        value: b"v",
      },
      ResponseHeader {
        name: b"x-ok",
        value: b"line\r\nbreak",
      },
      ResponseHeader {
        name: b"x-ok",
        value: b"del\x7f",
      },
    ] {
      let result = encode_head(
        &Budget::new(1 << 20),
        &mut DateCache::default(),
        1,
        200,
        &[header],
        Framing::Chunked,
        false,
        true,
      );
      assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
    let tab = head(
      1,
      200,
      &[ResponseHeader {
        name: b"x-tab",
        value: b"a\tb",
      }],
      Framing::Length(0),
      false,
      true,
    );
    assert!(tab.contains("x-tab: a\tb\r\n"));
  }

  #[test]
  fn encode_head_rejects_heads_that_exceed_the_budget() {
    let result = encode_head(
      &Budget::new(8),
      &mut DateCache::default(),
      1,
      200,
      &[],
      Framing::Length(0),
      false,
      true,
    );
    assert!(result.is_err());
  }

  #[test]
  fn reset_content_on_keep_alive_is_delimited_even_when_streamed() {
    // 205 is not a bodiless status for message framing (RFC 9112 §6.3): without a length a
    // keep-alive client must read until close, so the head has to advertise an empty body.
    let out = head(1, 205, &[], Framing::Chunked, false, true);
    assert!(
      out.contains("content-length: 0\r\n") || out.contains("connection: close\r\n"),
      "{out}"
    );
    let sized = head(1, 205, &[], Framing::Length(12), false, true);
    assert!(sized.contains("content-length: 0\r\n"), "{sized}");
  }

  #[test]
  fn date_cache_emits_a_well_formed_line() {
    let mut cache = DateCache::default();
    let line = cache.line().to_vec();
    assert_eq!(line.len(), DATE_LEN);
    assert!(line.starts_with(b"date: ") && line.ends_with(b" GMT\r\n"));
    assert_eq!(cache.line().len(), DATE_LEN);
  }

  #[test]
  fn frame_chunk_empty_final_is_only_the_terminator() {
    let mut buffer = Buffer::new(CHUNK_HEADROOM + CHUNK_TAIL, Budget::new(1 << 20)).unwrap();
    // SAFETY: This test initialized the payload in the owned buffer before framing it.
    let range = unsafe { frame_chunk(&mut buffer, 0, true) };
    assert_eq!(&buffer.as_init()[range], b"0\r\n\r\n");
    let mut buffer = Buffer::new(CHUNK_HEADROOM + 1 + CHUNK_TAIL, Budget::new(1 << 20)).unwrap();
    buffer.write(0, &[0; CHUNK_HEADROOM]).unwrap();
    buffer.write(CHUNK_HEADROOM, b"z").unwrap();
    // SAFETY: This test initialized the payload in the owned buffer before framing it.
    let range = unsafe { frame_chunk(&mut buffer, 1, false) };
    assert_eq!(&buffer.as_init()[range], b"1\r\nz\r\n");
  }
}
