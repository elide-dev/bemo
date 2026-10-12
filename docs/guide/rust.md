# Rust

The `bemo` crate is the transport core with no JVM, Netty, or GraalVM
dependency. Pin it by full commit:

```toml
[dependencies]
bemo = { git = "https://github.com/elide-dev/bemo", rev = "<full-commit-sha>" }
```

Add `bemo-ffi` at the same revision only if you also need the C exports.

## Features

| Feature | Effect |
| --- | --- |
| `bundled-mimalloc` (default) | Native buffers use a bundled mimalloc on non-Apple targets |
| `rust-allocator` | Native buffers use the Rust global allocator |
| neither | Buffers call external `mi_malloc`/`mi_free`, which your final link must provide |
| `fuzzing` | Unstable drivers for the fuzz workspace; not API |

Apple targets always use the Rust allocator, so a second mimalloc never shares
the host's TLS heap slot. Embedders that already link mimalloc should disable
default features.

## Modules

| Module | Contents |
| --- | --- |
| `bemo::buffer` | `Budget`, `Buffer`, and `FrozenBuffer`: budgeted native storage with retained slices |
| `bemo::driver` | Completion-based drivers over io_uring, polling, and IOCP |
| `bemo::tls` | `Session`, a Rustls state machine that performs no I/O |
| `bemo::http` | HTTP/1 request parsing (`HttpConnection`) and response encoding (`encode_head`, `encode_response`) |
| `bemo::compression` | `Gzip`, a reusable zlib-rs encoder |
| `bemo::alloc` | Raw allocations from the transport's allocator |
| `bemo::abi` | The handle-based operations behind the C exports |

## Driving `tls::Session`

`Session::step(input, action)` advances one transition and never touches a
socket, so you own scheduling. The caller's obligations:

- Remove `step.discard` bytes from the front of `input` before the next call.
- Send `step.output` buffers in order. Encoded handshake output must be
  acknowledged with `Action::Transmitted` once the transport has written it.
- Keep offering `Action::Transmitted` until a step returns
  `State::NeedTransmit`. A step that first surfaces plaintext or consumes
  records does not consume the acknowledgement; dropping it stalls the session.
- Any error is terminal: every later step fails.
- `step.plaintext` and `step.output` hold their own budgeted allocations, which
  are released when dropped.

The native HTTP lane in `bemo::http` drives sessions exactly this way, and the
`tls` fuzz target checks these rules against corrupted and fragmented traffic.

## Encoding responses

`encode_head` and `encode_response` reject statuses outside 100 to 999 and
invalid header names or values before allocating. Framing headers you pass
(`content-length`, `transfer-encoding`, `connection`, `date`, `server`,
`trailer`) are replaced by the encoder's own, so caller headers cannot produce
conflicting framing.
