# Extracted native transport

- This crate owns transport state and Rust handle operations; `dokar-ffi` alone
  owns unmangled C exports. Regenerate them with `tools/generate_exports.py`.
- Preserve Elide transport ABI 3 symbols, layouts, and workload ownership until
  a deliberate compatibility transition; `include/elide_transport.h` is shared
  by both JVM bindings. Run `make test` and `make test-native-image`.
- Keep the CompIO buf/driver/log type graph unified. The pinned CompIO fork
  directly pins polling; consumers must not need root Cargo patches.
- Preserve allocator pairing. Apple uses Rust allocation, not a second mimalloc;
  standalone non-Apple uses bundled mimalloc with dynamic TLS. Embedders providing
  mimalloc use `--no-default-features`.
- Cancellation is not completion. Retain kernel buffers/op keys until retirement,
  including IOCP and io_uring zero-copy notification completion.
- Driver state and poll operations are owner-thread confined. Release all
  registry borrows before callbacks; callbacks may respond, close, or release.
- Unknown/closed workloads start nothing. Accepted sockets inherit their listener
  workload. Every socket insert/removal must update the workload map.
- AUTO falls back only on classified io_uring setup failure. Keep original errno
  and preserve connection reset as error rather than EOF.
- Capture allowed affinity before spawning owners; helper restoration tokens are
  parent-specific and one-use. Never expand a captured affinity mask.
- Persistent receive windows charge pinned capacity; acknowledge body segments
  only after handoff, and release each segment once on the driver thread.
- Derive foreign-visible buffer addresses after reading geometry; later mutable
  reborrows can invalidate raw-pointer provenance.
- Tests that open drivers/sockets or invoke AWS-LC must keep their Miri exclusions.
  Select backends with `ELIDE_TRANSPORT_TEST_BACKEND`.
