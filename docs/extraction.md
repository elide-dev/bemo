# Elide extraction boundaries

The initial structure was informed by Elide revision
`68e31f11d9e4d194cc8b3fe4f644f9b9e5d9f155`. The Rust implementation and its
regression suite have now been extracted.
The Rust handle layer remains in `bemo::abi`, without unmangled exports;
`bemo-ffi` generates the C entrypoints from that layer and checks them against
`include/elide_transport.h`. This preserves crate-private state and ownership
invariants without exposing transport internals across crates.

`bemo-ffi` declares Cargo's `links = "bemo"` identity and supplies its header
directory as `DEP_BEMO_INCLUDE` to direct consumers' build scripts. Embedders
derive export lists and compiler includes from that pinned dependency instead
of keeping another header copy. Cargo rejects two independently versioned Bemo
FFI packages in one graph, preventing duplicate C boundaries.

| Elide source | Bemo destination | Constraints |
| --- | --- | --- |
| `crates/netty-transport/src/{buffer,driver,http,tls}*` | `crates/bemo` | Preserve runtime independence and per-crate feature choices |
| `crates/netty-transport/src/abi*` | `bemo::abi` + generated `bemo-ffi` exports | One foreign boundary; preserve ABI 3 symbols |
| `crates/netty-transport/include/elide_transport.h` | `include` | Preserve handle semantics, ownership rules, layouts, and workload parameters |
| `packages/base/main/dev/elide/netty/v2/TransportNative.java` | `packages/api` | No Elide runtime dependencies |
| `FfmTransportNative.java` | `packages/ffm` | JDK 22+ path; explicit library lifetime and ABI negotiation |
| `svm/CapiTransportNative.java` | `packages/native-image` | Replace `StacklessExceptions`, `Unsafe`, and Truffle annotation dependencies with standalone facilities |
| Channel, event-loop, buffer, and TLS adapters in `netty/v2` | `packages/netty` | Verify against stock Netty 4.2 from Maven Central |
| `NativeRegion.java` | Remains in Elide | Truffle-specific interop does not belong in the standalone adapter |
| `packages/base/main/io/netty/handler/ssl` | `packages/netty` | Package-private ALPN integration requires explicit compatibility coverage |
| `crates/netty-transport/tests` | Rust and shared binding contracts here | Preserve backend, shutdown, ownership, TLS, and reentrant-close cases |

Bemo pins CompIO at `029af1602c7701dd4fc607c9c857a26144153b14`, based on Elide's
`8feca49de69cb8090f18405741982b416a4beda9`. The two polling dependencies in that
fork now directly pin `1198249b4e54fa430dc6f76b058ad0912bd6bbea`. No source is
vendored. ntex-httparse is directly pinned at
`104a6749f9b8c973e99e9f84cfd2adcfdef96bbf`. Cargo
ignores a dependency's root `[patch]` entries when another workspace consumes
it. Put required forks directly into dependency declarations or use fork
releases with a consistent transitive graph. CompIO's buf/driver/log crates
must resolve together. Verify with the external Git consumer test, not only a
local workspace build. See [Cargo patch semantics](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html).

Preserve the optional bundled-mimalloc behavior when importing allocation code.
Apple must not link a second mimalloc into Elide; other standalone platforms
need the appropriate dynamic TLS behavior. Do not add a global allocator here
that overrides the embedding application.

The imported regression suite covers: owner/buffer accounting
and invalid handles; driver cancellation/completion lifetimes; Netty read/write
and reference counts; TLS/ALPN and shutdown; HTTP and workload admission. Force
polling and io_uring separately on Linux and exercise restricted io_uring setup
under seccomp. IOCP must retain buffers until cancellation completion.

Elide cutover is a later change in Elide: pin the Bemo Git revision, depend on
these JVM artifacts or their source during development, remove copied transport
implementations, and leave only runtime-specific integration. Do not maintain a
second independent copy of the transport after that cutover.

## Completed JVM extraction

The API, FFM, C API, Netty channel/buffer/TLS adapters, ALPN helper, reflection
metadata, and standalone Java contracts now live here. C API byte copies use
public GraalVM buffer views; standard exceptions replace Elide-specific helpers.
Neither Truffle annotations nor test-only runtime shims are shipped.

Both bindings pass the channel, allocator, TLS, callback, lifecycle, reentrant
close, JFR, and JSSE/OpenSSL contracts. CI qualifies JVM FFM on Linux and macOS
with JDK 22/25, Native Image on Linux, and Rust on Linux/macOS/Windows. Rust tests
cover HTTP/1, HTTP/2, native TLS, workload isolation, topology, and ownership.

Elide's working tree has not been changed. Its cutover must remove the old Rust
implementation and duplicate Java classes, including the ALPN helper, then
link `bemo-ffi` and consume `bemo` for Rust APIs. The Java package and native
symbol compatibility minimize source changes, but the native library name is
now `bemo_ffi`. Update Elide's library discovery/link directives accordingly.
Do not link both libraries into a process: they export identical ABI 3 symbols.

Align Elide's existing CompIO and polling source identities with these exact
revision pins during cutover; leaving old branch-based root patches can create
a second buffer/driver type graph even when the source revisions match. Use
`cargo tree -d` and the consumer test to verify the resulting graph.
