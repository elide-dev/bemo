# Embedding Bemo in Elide

This guide covers what an embedder such as Elide must do to link Bemo into its
own Rust and Native Image build, and which of those properties Bemo tests on its
behalf. The cutover itself happens in Elide; see [extraction](extraction.md) for
the history and the original source mapping.

## What Elide consumes

| Bemo piece | How Elide uses it |
| --- | --- |
| `bemo` (Rust crate) | Direct Rust API for transport, TLS, HTTP, and compression |
| `bemo-ffi` (Rust crate, `links = "bemo"`) | The C exports, linked into Elide's own static library |
| `include/` (through `DEP_BEMO_INCLUDE`) | Headers for Elide's Native Image C directives and export lists |
| `bemo-api`, `bemo-netty` | The Java interface and Netty adapters |
| `bemo-native-image` | `CapiTransportNative`, the C-call binding used in Elide's image |

Elide does not use `bemo-ffm` inside its native image. It must not ship the
`bemo_ffi` shared library alongside its own static copy, because both export
identical symbols.

## Cargo

Pin both crates to the same full commit, with default features off, so Elide's
allocator is used:

```toml
[dependencies]
bemo = { git = "https://github.com/elide-dev/bemo", rev = "<sha>", default-features = false }
bemo-ffi = { git = "https://github.com/elide-dev/bemo", rev = "<sha>", default-features = false }
```

- **Allocator.** With no allocator feature on a non-Apple target, Bemo calls
  `mi_malloc` and `mi_free` as external C symbols. Elide's mimalloc provides
  them, or on static musl the Elide toolchain's libc does, since it has mimalloc
  built in. Select `rust-allocator` instead to route through Elide's Rust
  `#[global_allocator]`. Never enable `bundled-mimalloc` inside Elide: it links a
  second mimalloc. Apple targets always use the Rust allocator.
- **Forks.** Bemo pins its CompIO, polling, ntex-httparse, and zlib-rs forks
  directly in its dependency declarations, so they reach Elide's workspace
  without `[patch]` entries. Remove any older root patches for those crates and
  check `cargo tree -d` for a second CompIO graph.
- **AWS-LC.** Keep `aws-lc-rs` and `aws-lc-sys` unified with Bemo's pins. Cargo
  merges one version into a single copy. Two different `aws-lc-sys` versions
  would both export a few unprefixed assembly symbols (`mldsa_ntt`,
  `mldsa_intt`, `mldsa_reduce`, `sha3_keccak4_f1600`) and fail to link. Neither
  collides with the toolchain's AWS-LC 5.7.0 `libcrypto.a`.
- **Headers.** `bemo-ffi`'s build script exports `DEP_BEMO_INCLUDE`. Read it from
  Elide's build script instead of copying headers.
- **One FFI per graph.** Cargo's `links = "bemo"` rejects two Bemo FFI packages
  in one build.

## What Bemo verifies for embedders

`make test-embedder` builds [`tests/embedder`](../tests/embedder), a standalone
crate shaped like Elide. It has its own workspace and its own allocator, and it
links `bemo-ffi` with default features off into its own static library. That
archive is then linked into a C program with the Elide toolchain's clang:

| Case | Target | Allocator behind Bemo's buffers |
| --- | --- | --- |
| `gnu-mimalloc` | glibc 2.34 | The embedder's own mimalloc |
| `gnu-rust-allocator` | glibc 2.34 | The embedder's `#[global_allocator]` (counted) |
| `musl-libc-mimalloc` | static musl | The mimalloc in the toolchain's musl libc |
| `musl-rust-allocator` | static musl | The embedder's `#[global_allocator]` (counted) |

Each case asserts that headers come from `DEP_BEMO_INCLUDE`, that every export
declared in `bemo.h` and `elide_transport.h` is present exactly once, that Bemo
bundles no mimalloc, and that buffer storage comes from the expected allocator.

`make test-musl` runs the whole Rust suite as fully static
`x86_64-unknown-linux-musl` executables linked by the toolchain. Bemo did not
compile for musl before this was qualified: it built `msghdr` with struct
literal syntax, which musl's padding fields forbid.

`make test-asan` and `make test-tsan` run the transport, AWS-LC, and the C and
FFM consumers under the toolchain's sanitizer runtime (see
[native safety](native-safety.md)).

## Toolchain

Elide builds with the [Elide toolchain](https://github.com/elide-dev/toolchain)
(LLVM 23, glibc 2.34 and static musl sysroots). Bemo pins the same release, and
its sanitizer add-ons, in `tools/versions.json`; `python3 tools/setup_llvm.py`
installs it under `build/tools/`. rustc's LLVM major must not exceed the
toolchain's (`llvmMajor` in its manifest). ThinLTO bitcode archives
(`make build-bitcode`) let Elide's final link optimize across the C boundary;
see [generated seams](generated-seam.md).

## Netty memory from Bemo's allocator

With `CAP_NATIVE_ALLOC`, the bindings expose `bemo_alloc`, `bemo_free`, and
`bemo_alloc_live`. These are raw allocations from exactly the allocator the
transport uses (bundled mimalloc, or the embedder's), not charged to any
transport owner. `NativeDirectByteBufAllocator` builds on them so that Netty
direct buffers are allocated, grown, and freed through that allocator instead of
the JDK or Netty's cleaner:

```java
var allocator = new NativeDirectByteBufAllocator(transport);
bootstrap.childOption(ChannelOption.ALLOCATOR, allocator);
long live = allocator.nativeAllocatedBytes(); // process-wide, for metrics and leak checks
```

The allocator is opt-in, unpooled in Java (mimalloc already caches per thread),
and releases buffers correctly on any thread. It uses Netty 4.2's
`CleanableDirectBuffer` hook, so it is tied to the Netty minor version Bemo
qualifies. `NativeByteBufAllocator` remains the choice when allocations must be
charged to a transport owner's budget.

## Native Image

Link Bemo's exports from Elide's static library and use
`dev.elide.bemo.transport.svm.CapiTransportNative`. The `bemo-netty` JAR carries
Native Image configuration enabling shared arenas, which Netty's Java 25
direct-buffer cleaner requires. Native Image executables cannot be sanitized;
the sanitizer stages cover the same Rust code through C and JVM consumers.

## What remains in Elide

- `NativeRegion` (Truffle interop) stays in Elide.
- Delete Elide's copied transport crate, Java classes, and ALPN helper when the
  cutover lands. The native library name is `bemo_ffi`; update discovery and
  link directives.
- Observability: Bemo will emit structured events, spans, and metrics, and
  Elide supplies the OpenTelemetry bridge. See the
  [observability design](superpowers/specs/2026-10-11-transport-observability-design.md).
