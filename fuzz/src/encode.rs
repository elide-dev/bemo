use bemo::buffer::Budget;
use bemo::compression::Gzip;
use bemo::http::{DateCache, Framing, ResponseHeader, encode_head};
use std::io::Read;

const OWNED: [&[u8]; 6] = [
  b"content-length",
  b"connection",
  b"date",
  b"server",
  b"transfer-encoding",
  b"trailer",
];

fn owned(name: &[u8]) -> bool {
  OWNED.iter().any(|owned| name.eq_ignore_ascii_case(owned))
}

/// Encoded response heads must parse with a reference parser into exactly the caller's
/// non-owned headers, in order, plus one framing header consistent with the request.
pub fn response(data: &[u8]) {
  let [version, status_high, status_low, flags, rest @ ..] = data else {
    return;
  };
  let version = version & 1;
  let status = u16::from_be_bytes([*status_high, *status_low]);
  let framing = match flags & 3 {
    0 => Framing::Length(u64::from(flags >> 2) * 1000),
    1 => Framing::Chunked,
    _ => Framing::Close,
  };
  let head_only = flags & 0x40 != 0;
  let keep_alive = flags & 0x80 != 0;
  // Fields are NUL-separated name/value pairs so mutations can reach every byte value.
  let fields: Vec<&[u8]> = rest.split(|byte| *byte == 0).collect();
  let headers: Vec<ResponseHeader<'_>> = fields
    .as_chunks::<2>()
    .0
    .iter()
    .take(64)
    .map(|[name, value]| ResponseHeader { name, value })
    .collect();
  let budget = Budget::new(1 << 20);
  let encoded = encode_head(
    &budget,
    &mut DateCache::default(),
    version,
    status,
    &headers,
    framing,
    head_only,
    keep_alive,
  );
  let valid_name = |name: &[u8]| {
    !name.is_empty()
      && name
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
  };
  let valid_value = |value: &[u8]| value.iter().all(|b| *b == b'\t' || (*b >= b' ' && *b != 0x7f));
  let acceptable = (100..1000).contains(&status)
    && headers
      .iter()
      .all(|header| valid_name(header.name) && valid_value(header.value));
  let Ok(encoded) = encoded else {
    assert!(!acceptable, "valid response head rejected");
    assert_eq!(budget.used(), 0);
    return;
  };
  assert!(acceptable, "invalid response head accepted");
  let wire = encoded.as_ref();
  assert!(wire.ends_with(b"\r\n\r\n"));
  let mut parsed = [httparse::EMPTY_HEADER; 80];
  let mut response = httparse::Response::new(&mut parsed);
  let parsed_len = match response.parse(wire) {
    Ok(httparse::Status::Complete(length)) => length,
    other => panic!("reference parser rejected the encoded head: {other:?}"),
  };
  // No header value may smuggle a second head or a premature blank line.
  assert_eq!(parsed_len, wire.len(), "encoded head ended early");
  assert_eq!(response.code, Some(status));
  assert_eq!(response.version, Some(version));
  let caller: Vec<(&[u8], &[u8])> = headers
    .iter()
    .filter(|header| !owned(header.name))
    .map(|header| (header.name, header.value))
    .collect();
  let emitted: Vec<(&[u8], &[u8])> = response
    .headers
    .iter()
    .filter(|header| !owned(header.name.as_bytes()))
    .map(|header| (header.name.as_bytes(), header.value))
    .collect();
  // httparse trims trailing whitespace in values; compare the trimmed forms.
  let trim = |value: &[u8]| value.trim_ascii().to_vec();
  assert_eq!(
    caller.iter().map(|(n, v)| (n.to_vec(), trim(v))).collect::<Vec<_>>(),
    emitted.iter().map(|(n, v)| (n.to_vec(), trim(v))).collect::<Vec<_>>(),
    "caller headers were not preserved in order"
  );
  let count = |name: &[u8]| {
    response
      .headers
      .iter()
      .filter(|h| h.name.as_bytes().eq_ignore_ascii_case(name))
      .count()
  };
  let lengths = count(b"content-length");
  let chunked = count(b"transfer-encoding");
  assert!(lengths + chunked <= 1, "conflicting or duplicate framing headers");
  assert_eq!(count(b"date"), 1);
  assert_eq!(count(b"server"), 1);
  assert!(count(b"connection") <= 1);
  let bodiless = status == 204 || status == 304 || (100..200).contains(&status);
  if bodiless {
    assert_eq!(lengths + chunked, 0, "bodiless status carries framing");
  }
  if version == 0 || head_only {
    assert_eq!(chunked, 0, "chunked framing on HTTP/1.0 or HEAD");
  }
  drop(encoded);
  assert_eq!(budget.used(), 0);
}

/// Each gzip member is independently decodable to its input, encoders are reusable
/// across inputs, and output storage is charged to and released from the budget.
pub fn gzip(data: &[u8]) {
  let [level, split, rest @ ..] = data else {
    return;
  };
  let Ok(mut encoder) = Gzip::new(u32::from(*level % 10)) else {
    panic!("levels 0..=9 are valid");
  };
  assert!(Gzip::new(10).is_err());
  let at = usize::from(*split) * rest.len() / 256;
  let budget = Budget::new(4 << 20);
  let mut members = Vec::new();
  for input in [&rest[..at], &rest[at..], rest] {
    let member = encoder.encode(input, &budget).expect("bounded input compresses");
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(member.as_ref())
      .read_to_end(&mut decoded)
      .expect("reference decoder accepts the member");
    assert_eq!(decoded, input, "gzip round trip changed the input");
    members.push(member);
  }
  let charged: usize = members.iter().map(|member| member.as_ref().len()).sum();
  assert!(budget.used() >= charged);
  drop(members);
  assert_eq!(budget.used(), 0);
}
