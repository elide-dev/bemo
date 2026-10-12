# Overview

Bemo replaces the native layers underneath a Netty application with Rust:

| Netty piece | Bemo replacement |
| --- | --- |
| epoll / kqueue / NIO transport | `NativeIoHandler` and native channels over io_uring, epoll, kqueue, or IOCP |
| JDK SSLEngine or netty-tcnative | [Rustls](https://github.com/rustls/rustls) with [AWS-LC](https://github.com/aws/aws-lc-rs) cryptography |
| JDK `Deflater` | [zlib-rs](https://github.com/trifectatechfoundation/zlib-rs) gzip |
| Direct-buffer allocation | Native buffers from mimalloc, or the embedder's allocator |

Applications keep their pipelines, handlers, and HTTP codecs. Choosing a Bemo
channel replaces only the transport. TLS, compression, and buffer allocation are
each opted into separately, so one dependency does not change all four.

## Integration paths

| Path | You use | Bemo provides |
| --- | --- | --- |
| Netty on the JVM (JDK 22+) | `bemo-netty` + `bemo-ffm` + a platform classifier | FFM calls into a shared library loaded from the classpath |
| Netty in GraalVM Native Image | `bemo-netty` + `bemo-native-image` + a static classifier | C calls into a statically linked library |
| Native HTTP server | The C ABI's HTTP functions | HTTP/1.1 and HTTP/2 parsing, encoding, and TLS in Rust; your code handles exchanges |
| Rust | The `bemo` crate | The same core without any JVM |
| Embedding (Elide) | `bemo-ffi` linked into your own static library | Exports, headers, and allocator integration |

The native HTTP path is separate from Netty: it parses requests and encodes
responses natively and hands the JVM only exchange handles and byte views. See
the [native HTTP workload](../native-http-example.md).

## Platform support

| Platform | Rust core | JVM packages | Native Image |
| --- | --- | --- | --- |
| Linux x86-64, glibc | Tested | Published (`linux-x86_64-gnu`, glibc 2.39) | Published |
| macOS 15, ARM64 | Tested | Published (`osx-aarch64`) | Published |
| Linux x86-64, static musl | Tested with the Elide toolchain | Not published | Through embedding |
| Windows x86-64 | Tested | Not published | Not published |
| Linux ARM64, Intel macOS | Not qualified | Not published | Not published |

Netty 4.2 is required. Netty TLS integration requires the classpath rather than
the module path.

## How Bemo is verified

Every change runs the Rust suite on Linux, macOS, and Windows, and the shared
Java contract through both bindings. Native safety runs add ASAN and TSAN across
Rust, AWS-LC, the C consumers, and the FFM contracts in a stock JVM, plus Miri
and fuzzing of the HTTP, HTTP/2, TLS, response-encoding, gzip, buffer, and
handle code. Details are in [native safety](../native-safety.md).
