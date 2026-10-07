# Incremental HTTP head progress

A connection retains the request-line metadata, complete header spans, framing
hints, and cursor while accumulating a fragmented HTTP/1 head. Each complete
line is validated once. An incomplete request or header line still goes through
`ntex_httparse` on every fragment, preserving early rejection without adding a
second validator or waiting for the complete head terminator. Large individual
lines can still require repeated scans; this change removes repeated work on
previously completed lines.

The accumulator initially reserves at least 128 bytes to avoid successive tiny
vector growth for common fragmented heads. Complete heads still use receive
storage directly; fragmented heads copy into owned storage once. Delivered
exchanges own their header vectors. Progress resets on head completion or error.
Trailer and body framing continue using their existing paths and limits.

An independent copy of the previous head parser serves as a test oracle at every
prefix of valid and invalid requests, including incomplete malformed fields,
header-count limits, and oversized fields. Every fragment size of a body plus a
pipelined request verifies paths, header spans, body bytes, and one body end.
The existing HTTP framing and smuggling regression suite remains in force.

Criterion covers contiguous, seven-byte, and one-byte input with both new and
persistent connections. Compare against the immediate preceding PR using the
same benchmark definitions to separate this change from allocation reuse.

Local ARM64 macOS probes (20 samples, 200 ms warmup, 500 ms measurement)
showed lower time for fragmented requests against PR 3: approximately 10% for
seven-byte GET, 21% for POST, and 32% for chunked input; one-byte cases improved
approximately 17–27%. These include receive-buffer construction. Contiguous
measurements were near the prior GET/chunked values, while POST samples were
noisier. These short local probes are not an end-to-end claim; qualify them with
CodSpeed and compare persistent and newly constructed connections separately.
