# Retained response bodies

`CHUNK_RETAIN` (flags bit 2) lets `http_chunk_send` take an immutable lease of
an existing frozen handle, without copying its initialized bytes. The caller
keeps the handle on every result, including success, busy, invalid arguments,
and declared-length failure. The driver holds its own lease until queued and
in-flight references retire. It is safe to release the caller's handle or free
the exchange after queueing a final part; neither operation retires kernel I/O.

The payload starts at offset zero and its length cannot exceed the frozen
view's initialized length. HTTP/1 declared-length and close-delimited streaming
responses support this flag, as does HTTP/2. HTTP/1 chunked framing still needs
prepared mutable parts to write framing in place and rejects retained parts.
There is no ABI version, symbol, or layout change. Both Java bindings forward
the shared flag unchanged.

The HTTP benchmark now uploads identity bodies once and retains them per
response. Dynamic gzip still compresses each response, then copies the result
once into a frozen native body. The former buffered response copied that body
again into combined head/body storage.

A local ARM64 macOS Criterion probe (10 samples, 100 ms warmup, 200 ms
measurement) measured 64 KiB combined response encoding around 2.1 microseconds
and head encoding plus an immutable body slice around 72 ns. These are different
encoding strategies, not an end-to-end or full ABI measurement. Transport
completion, registry lookup, compression, and network costs are excluded.

The shared JVM/Native Image contract sends pipelined responses, releases the
caller handle and exchanges before completion, checks exact wire bodies, and
checks storage remains charged until retirement. Rust contracts additionally
cover mutable-handle rejection, initialized bounds, busy retry ownership,
chunked rejection, declared-length errors, and HTTP/2 physical acknowledgement.
