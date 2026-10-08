![Bemo](./docs/images/banner.png)

[![Discord](https://img.shields.io/discord/1119121740161884252?b2&logo=discord&logoColor=white&label=Discord)](https://elide.dev/discord)
![Java 21+](https://img.shields.io/badge/-Java%2021%2B-blue.svg?logo=openjdk&logoColor=white)
![Rust 2024](https://img.shields.io/badge/-Rust%202024-orange.svg?logo=rust&logoColor=white)
[![codecov](https://codecov.io/gh/elide-dev/bemo/graph/badge.svg?token=gWxa2IrgIO)](https://codecov.io/gh/elide-dev/bemo)
[![CodSpeed](https://img.shields.io/endpoint?url=https://codspeed.io/badge.json)](https://codspeed.io/elide-dev/bemo)

> A [_Bemo_][0] is a traditional, small open-air minibus or motorized rickshaw used as fast local **transport, especially in [Bali](https://github.com/elide-dev/bali)**.

---

# Netty fortified by Rust

_Bemo_ is a drop-in [native transport](https://netty.io/wiki/native-transports.html) for [Netty](https://netty.io), built with Rust, using best-of-breed APIs and libraries like [`io_uring`](https://en.wikipedia.org/wiki/Io_uring), [`tokio`](https://tokio.rs/), [`aws-lc-rs`](https://github.com/aws/aws-lc-rs), [`rustls`](https://github.com/rustls/rustls), [`zlib-rs`](https://trifectatech.org/blog/zlib-rs-is-faster-than-c/), [`ntex`](https://ntex.rs/), and [`simdutf`](https://github.com/simdutf/simdutf).

| Status | Feature |
| ------ | ------- |
| ✅ Drop-in | Replacement for Netty native transports (`io_uring`, `epoll`, `kqueue`) |
| ✅ Drop-in | Replacement for Netty "Tomcat Native" (`tcnative`) TLS |
| ✅ JVM | FFM binding with shared transport, TLS, and compression contracts |
| ✅ Native Image | Statically linked C binding with shared contracts |

_Bemo_ is used as the main transport for [Elide](https://github.com/elide-dev/elide).

## Usage

**Via Rust:**

```toml
[dependencies]
bemo = { git = "https://github.com/elide-dev/bemo", rev = "<full-commit-sha>" }
bemo-ffi = { git = "https://github.com/elide-dev/bemo", rev = "<full-commit-sha>" }
```

**Via JVM/SVM:**

The Maven coordinates are `dev.elide.bemo:bemo-api`, `bemo-ffm`,
`bemo-native-image`, and `bemo-netty`. Snapshot artifacts, sources, Javadocs,
and native classifiers are available from [GitHub Packages](docs/publishing.md). The FFM and
Native Image artifacts depend on the API artifact. GraalVM SDK dependencies
are confined to the Native Image artifact and marked `provided`.

```java
import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.NativeIoHandler;
import dev.elide.bemo.transport.NativeServerSocketChannel;
import dev.elide.bemo.transport.Workload;
import io.netty.channel.MultiThreadIoEventLoopGroup;

var transport = new FfmTransportNative();
var group = new MultiThreadIoEventLoopGroup(
    2, NativeIoHandler.newFactory(transport, 0, 128, 8 * 1024 * 1024));
// Supply group and NativeServerSocketChannel.class to a Netty ServerBootstrap.
// After channels are closed:
group.shutdownGracefully().sync();
Workload.close(transport, Workload.DEFAULT);
```

For Native Image, construct `dev.elide.bemo.transport.svm.CapiTransportNative` instead.
The transport binding retains its library for process lifetime; caller-owned
workloads and event loops have explicit shutdown. See the executable contracts
in `tests/transport/java` for complete TCP, Unix socket, and TLS examples.

Run with `--enable-native-access=ALL-UNNAMED`. Include the base FFM JAR and its
platform classifier JAR; the no-argument constructor extracts and loads the
shared library automatically. Static archives ship in the Native Image classifier.
See [native loading](docs/native-loading.md) for configuration. Netty TLS uses
a package-private ALPN adapter and currently requires the classpath rather than JPMS. See [architecture](docs/architecture.md), [extraction boundaries](docs/extraction.md),
[Netty I/O ownership and batching](docs/transport-io.md).

## Performance

The [native-baseline cycle](docs/performance/native-baseline.md)
compares against **Netty epoll and tcnative/BoringSSL**, with **gzip level 1**
and **TLS 1.3 / AES-128-GCM** on both sides. It retains three samples for every
workload: 72 basic samples and 180 framework samples, including Ktor in JVM
and Native Image modes. Responses are validated throughout.

Gzip is Bemo’s strongest measured workload: **2.4–2.8×** the stock JVM framework
throughput and **4.1–7.6×** the stock Native Image throughput. Large uncompressed
JVM framework payloads remain **15–20% slower**. Native Image framework TLS is
near parity (**−2.7% to +0.6%**); the archived 20×-plus TLS advantage disappears
against this native baseline. These are closed-loop loopback results on a
shared Threadripper PRO 9965WX host; sample ranges describe observed variation.

The basic matrix compares complete server stacks: Bemo Native Image `-O3`,
native HTTP, io_uring and Rustls/aws-lc-rs against OpenJDK Netty epoll, Netty
HTTP and tcnative/BoringSSL. Bemo has higher median throughput in ten of twelve
workloads; uncompressed TLS at 64 KiB and 128 KiB is **15% and 12% slower**.
Both stacks use the same external OpenJDK NIO/JSSE client.

![Throughput across twelve HTTP and TLS workloads, with three-sample ranges](docs/performance/graphs/throughput-0fe6f875e1f9.svg)

![Identity throughput and p99 latency at 1 KiB, 64 KiB, and 128 KiB](docs/performance/graphs/payload-curves-0fe6f875e1f9.svg)

![Server CPU and combined server/client memory across all workloads](docs/performance/graphs/efficiency-0fe6f875e1f9.svg)

Framework comparisons hold the runtime fixed within each row and retain the
framework HTTP codecs. Bemo supplies io_uring, zlib-rs and Rustls; stock mode
uses epoll, a reusable JDK Deflater and tcnative/BoringSSL. Native Images use
`-O3`, the portable `x86-64-v3` target and no trained PGO profile. Each sample
uses 64 connections, 20 seconds of warmup and 20 seconds measured.

![Spring Boot, Micronaut and Ktor throughput across five endpoints in JVM and Native Image modes](docs/performance/graphs/framework-throughput-4c1e3556f5cc.svg)

Bemo median throughput change versus native Netty:

| Framework | Runtime | HTTP 13 B | HTTP 128 KiB | Gzip 128 KiB | TLS 128 KiB | TLS+gzip 128 KiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | -0.9% | -15.1% | +141.6% | -2.6% | +126.2% |
| Spring Boot | Native Image | +3.4% | +0.1% | +474.0% | +0.6% | +420.4% |
| Micronaut | JVM | -6.8% | -20.4% | +183.4% | -12.7% | +171.1% |
| Micronaut | Native Image | +1.0% | -10.6% | +664.9% | -2.6% | +590.9% |
| Ktor | JVM | -7.0% | -20.4% | +183.2% | -7.3% | +165.8% |
| Ktor | Native Image | -3.1% | +18.5% | +305.6% | -2.7% | +259.5% |

Equal gzip levels produce different sizes: the 128 KiB framework ASCII body is
**1,580 bytes with Bemo versus 909 bytes with stock gzip**. For the basic JSON
body, the sizes are **1,653 versus 1,002 bytes**; Netty level 6 produces **479
bytes** as an untimed size control. Throughput gains retain this wire-size cost.

Ranges are sample minima and maxima, not confidence intervals. Basic memory
sums server/client lifetime RSS high-water marks; framework memory is
server-only. The harnesses use different bodies, clients and concurrency, so
their absolute rates cannot be compared directly. The report retains CPU,
latency, memory, source patches, toolchains, artifact hashes and raw samples.
The [NIO/JDK TLS report](docs/performance/unclemax-matched.md) remains archived.
Run `make bench-graphs` to reproduce the current charts; see the
[chart refresh instructions](docs/performance/README.md).

## Layout

| Path | Responsibility |
| --- | --- |
| `crates/bemo` | Runtime-independent Rust core; consumable as a Cargo Git dependency |
| `crates/bemo-ffi` | One C boundary, built as `rlib`, `staticlib`, and `cdylib` |
| `include` | Bemo metadata ABI and preserved Elide transport ABI 3 |
| `packages/api` | JVM contract without runtime dependencies |
| `packages/ffm` | JDK 22+ dynamic-library adapter |
| `packages/native-image` | GraalVM C interop adapter for static linking |
| `packages/netty` | Stock Netty 4.2 channels, allocator, TLS, and event-loop adapter |
| `tests` | C and shared JVM binding contracts |
| `tools` | Build orchestration and artifact/consumer verification |
| `.github` | Reusable PR, push, merge-queue, check, build, and release-staging workflows |

## Develop

Install Rustup, Python 3.11+, a C toolchain, a JDK 25 build toolchain, and the
Elide version in `.elide-version`. Rustup selects `rust-toolchain.toml`.
Use a GraalVM JDK 25 with `native-image` for the static binding test. AWS-LC
requires CMake; Windows additionally needs NASM or `AWS_LC_SYS_PREBUILT_NASM=1`.
OpenSSL on PATH enables additional TLS interoperability checks.
The FFM artifact targets regular JDK 22+, without preview features.

```sh
make deps
make build
make check
make test
make test-native-image
make package
python3 tools/verify_package.py
```

`make test` covers Rust, an external revision-pinned Cargo Git consumer, a C
consumer, and FFM on the JVM. `make test-native-image` compiles and executes the
same JVM contract against the static C binding. CI tests FFM on Temurin 22 and
25; Rust on Linux, macOS, and Windows; Native Image on Linux. JVM packaging is
initially Linux glibc x86-64 and macOS ARM64. Windows JVM and musl packaging are
not yet qualified.

Set `ELIDE` to select the build executable, `JAVA_HOME` for JVM tools, or
`BEMO_TEST_JAVA` to run the FFM contract with a separate stock JVM. Standard
Cargo `CARGO_TARGET_DIR` is supported. Cross-compilation is not wired into the
host binding tests or packaging commands. Build commands share output folders;
run them sequentially in a checkout.

## Licensing

Licensed under Apache-2.0.

Test XML, coverage, and continuous CPU/RPS/RSS benchmarks are described in
[the measurement guide](docs/measurement.md).
ASAN, TSAN, Miri, and bounded native fuzzing are described in
[native safety verification](docs/native-safety.md).

Minimal Spring Boot, Micronaut, and Ktor applications support JVM and Native Image builds
with Elide, Maven, and Gradle, including native gzip, TLS, and TLS+gzip workloads.
See [the framework examples guide](docs/framework-examples.md).

[0]: https://en.wikipedia.org/wiki/Share_taxi#Indonesia
