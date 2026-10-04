/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

//! HTTP/1.x on transport v2: requests are parsed off receive completions into
//! [`Exchange`]s that borrow the received storage, and responses are encoded
//! natively. The JVM only ever sees exchange handles and pointer/length views.

mod encode;
pub(crate) mod h2;
mod parse;
pub(crate) mod tls;

pub use encode::{
  CHUNK_HEADROOM, CHUNK_TAIL, DateCache, Framing, ResponseHeader, encode_head, encode_response, frame_chunk,
};
pub use parse::{Connection as HttpConnection, Exchange, HeaderSpan, Method, Outcome, ParseError};

/// Largest request head (request line + headers) accepted, in bytes.
pub const MAX_HEAD_BYTES: usize = 16 * 1024;
/// Most headers accepted on one request.
pub const MAX_HEADERS: usize = 100;
/// Pinned receive capacity per connection; body segments are slices of this window.
pub const BODY_WINDOW_BYTES: usize = 256 * 1024;
/// Longest chunk-size line (size, extensions, CRLF) accepted before answering 400.
pub const MAX_CHUNK_LINE_BYTES: usize = 1024;
