# Elide extraction boundaries

The initial structure was informed by Elide revision
`68e31f11d9e4d194cc8b3fe4f644f9b9e5d9f155`. No transport implementation has been
copied in this foundation pass. Recheck the source revision before extraction.

| Elide source | Dokar destination | Constraints |
| --- | --- | --- |
| `crates/netty-transport/src/{buffer,driver,http,tls}*` | `crates/dokar` | Preserve runtime independence and per-crate feature choices |
| `crates/netty-transport/src/abi*` | `crates/dokar-ffi` | One foreign boundary; decide compatibility with old symbols before renaming |
| `crates/netty-transport/include/elide_transport.h` | `include` | Preserve handle semantics, ownership rules, layouts, and workload parameters |
| `packages/base/main/dev/elide/netty/v2/TransportNative.java` | `packages/api` | No Elide runtime dependencies |
| `FfmTransportNative.java` | `packages/ffm` | JDK 22+ path; explicit library lifetime and ABI negotiation |
| `svm/CapiTransportNative.java` | `packages/native-image` | Replace `StacklessExceptions`, `Unsafe`, and Truffle annotation dependencies with standalone facilities |
| Channel, event-loop, buffer, and TLS adapters in `netty/v2` | `packages/netty` | Verify against stock Netty 4.2 from Maven Central |
| `NativeRegion.java` | Remains in Elide | Truffle-specific interop does not belong in the standalone adapter |
| `packages/base/main/io/netty/handler/ssl` | Evaluate with Netty TLS adapter | Package-private ALPN integration requires explicit compatibility coverage |
| `crates/netty-transport/tests` | Rust and shared binding contracts here | Preserve backend, shutdown, ownership, TLS, and reentrant-close cases |

Elide currently uses pinned CompIO, polling, and ntex-httparse forks. Cargo
ignores a dependency's root `[patch]` entries when another workspace consumes
it. Put required forks directly into dependency declarations or use fork
releases with a consistent transitive graph. CompIO's buf/driver/log crates
must resolve together. Verify with the external Git consumer test, not only a
local workspace build. See [Cargo patch semantics](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html).

Preserve the optional bundled-mimalloc behavior when importing allocation code.
Apple must not link a second mimalloc into Elide; other standalone platforms
need the appropriate dynamic TLS behavior. Do not add a global allocator here
that overrides the embedding application.

Data-plane migration should bring tests with each slice: owner/buffer accounting
and invalid handles; driver cancellation/completion lifetimes; Netty read/write
and reference counts; TLS/ALPN and shutdown; HTTP and workload admission. Force
polling and io_uring separately on Linux and exercise restricted io_uring setup
under seccomp. IOCP must retain buffers until cancellation completion.

Elide cutover is a later change in Elide: pin the Dokar Git revision, depend on
these JVM artifacts or their source during development, remove copied transport
implementations, and leave only runtime-specific integration. Do not maintain a
second independent copy of the transport after that cutover.
