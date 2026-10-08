![Bemo](./docs/images/banner.png)

[![Discord](https://img.shields.io/discord/1119121740161884252?b2&logo=discord&logoColor=white&label=Discord)](https://elide.dev/discord)
![JDK 22+](https://img.shields.io/badge/-JDK%2022%2B-blue.svg?logo=openjdk&logoColor=white)
![Rust 2024](https://img.shields.io/badge/-Rust%202024-orange.svg?logo=rust&logoColor=white)
[![codecov](https://codecov.io/gh/elide-dev/bemo/graph/badge.svg?token=gWxa2IrgIO)](https://codecov.io/gh/elide-dev/bemo)
[![CodSpeed](https://img.shields.io/endpoint?url=https://codspeed.io/badge.json)](https://codspeed.io/elide-dev/bemo)

# Netty, powered by Rust

Bemo adds native I/O, [Rustls](https://github.com/rustls/rustls) TLS with
[AWS-LC](https://github.com/aws/aws-lc-rs), and
[zlib-rs](https://github.com/trifectatechfoundation/zlib-rs) compression to
[Netty](https://netty.io). It runs on the JVM through FFM or links statically
into GraalVM Native Image. Your application keeps its Netty pipelines and handlers.

## Usage

Bemo **0.3.0** is on Maven Central. For a Netty application on **JDK 22+**, add
the dependencies below and run with `--enable-native-access=ALL-UNNAMED`.
The examples use Linux x86-64 with glibc. On macOS ARM64, replace
`linux-x86_64-gnu` with `osx-aarch64`.

<details open>
<summary><strong>Gradle</strong> (build.gradle.kts)</summary>

```kotlin
repositories { mavenCentral() }

dependencies {
  implementation("dev.elide.bemo:bemo-netty:0.3.0")
  implementation("dev.elide.bemo:bemo-ffm:0.3.0")
  runtimeOnly("dev.elide.bemo:bemo-ffm:0.3.0:linux-x86_64-gnu")
}
```

</details>

<details>
<summary><strong>Maven</strong> (pom.xml)</summary>

Maven uses Central by default. Add these dependencies inside `<project>`:

```xml
<properties>
  <bemo.version>0.3.0</bemo.version>
</properties>

<dependencies>
  <dependency>
    <groupId>dev.elide.bemo</groupId>
    <artifactId>bemo-netty</artifactId>
    <version>${bemo.version}</version>
  </dependency>
  <dependency>
    <groupId>dev.elide.bemo</groupId>
    <artifactId>bemo-ffm</artifactId>
    <version>${bemo.version}</version>
  </dependency>
  <dependency>
    <groupId>dev.elide.bemo</groupId>
    <artifactId>bemo-ffm</artifactId>
    <version>${bemo.version}</version>
    <classifier>linux-x86_64-gnu</classifier>
    <scope>runtime</scope>
  </dependency>
</dependencies>
```

</details>

<details>
<summary><strong>Gradle version catalog</strong></summary>

In `gradle/libs.versions.toml`:

```toml
[versions]
bemo = "0.3.0"

[libraries]
bemo-netty = { module = "dev.elide.bemo:bemo-netty", version.ref = "bemo" }
bemo-ffm = { module = "dev.elide.bemo:bemo-ffm", version.ref = "bemo" }
```

In `build.gradle.kts`:

```kotlin
repositories { mavenCentral() }

dependencies {
  implementation(libs.bemo.netty)
  implementation(libs.bemo.ffm)
  runtimeOnly(variantOf(libs.bemo.ffm) { classifier("linux-x86_64-gnu") })
}
```

The classifier belongs in the build script; [version catalogs](https://docs.gradle.org/current/userguide/version_catalogs.html#sec:classifiers-artifact-types-capabilities)
store the module and version.

</details>

Bemo loads the native library from the classifier JAR automatically. See the
[package guide](docs/publishing.md) for platform requirements and Native Image
linking. The [Spring Boot, Micronaut, and Ktor examples](docs/framework-examples.md)
each include Elide, Maven, and Gradle builds.

### Java and Netty

Create an event-loop group and use Bemo's server channel:

```java
import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.NativeIoHandler;
import dev.elide.bemo.transport.NativeServerSocketChannel;
import dev.elide.bemo.transport.Workload;
import io.netty.channel.MultiThreadIoEventLoopGroup;

var transport = new FfmTransportNative();
var group = new MultiThreadIoEventLoopGroup(
  2, NativeIoHandler.newFactory(transport, 0, 128, 8 * 1024 * 1024));
// Supply group and NativeServerSocketChannel.class to your ServerBootstrap.

// After closing your channels, release the event loops and default workload:
group.shutdownGracefully().sync();
Workload.close(transport, Workload.DEFAULT);
```

For Native Image, use `dev.elide.bemo.transport.svm.CapiTransportNative` and
the static library from the `bemo-native-image` platform classifier. See the
[static linking instructions](docs/publishing.md#static-linkage).

The [transport examples](tests/transport/java) cover TCP, Unix sockets, and TLS,
including setup and shutdown. Netty TLS currently requires the classpath rather
than JPMS. For library paths and extraction settings, see
[native loading](docs/native-loading.md).

### Rust

To use the Rust core directly, pin a full commit SHA:

```toml
[dependencies]
bemo = { git = "https://github.com/elide-dev/bemo", rev = "<full-commit-sha>" }
```

For C exports, add `bemo-ffi` at the same revision. Public headers live in
[`include`](include); the `bemo` crate itself has no JVM or Netty dependency.

## Performance

These charts compare Bemo with **Netty epoll and tcnative/BoringSSL** across
HTTP, TLS, and gzip workloads. The [benchmark run](docs/performance/native-baseline.md)
predates the transport optimizations in 0.3.0.

The server comparison uses Bemo Native Image and Netty on OpenJDK. These results
include both transport and runtime differences.

![HTTP and TLS throughput across twelve workloads](docs/performance/graphs/throughput-0fe6f875e1f9.svg)

The framework comparison keeps the runtime and framework HTTP codecs the
same within each pair:

![Spring Boot, Micronaut, and Ktor throughput on the JVM and Native Image](docs/performance/graphs/framework-throughput-4c1e3556f5cc.svg)

In this run, gzip throughput was **2.4 to 2.8×** the stock JVM
framework throughput and **4.1 to 7.6×** the stock Native Image throughput.
Large uncompressed JVM responses were **15 to 20% slower**. Both sides used gzip
level 1, but Bemo produced larger compressed bodies: **1,580 vs. 909 bytes**
for the 128 KiB framework payload.

These are loopback measurements on a shared Linux host, with three samples per
workload; chart ranges show the observed minimum and maximum. The
[benchmark reports](docs/performance/README.md) include the methodology,
latency, CPU, memory, raw results, and subsequent measurements.

## Working on Bemo

Install Rustup, Python 3.11+, a C toolchain, CMake, a JDK 25 build toolchain,
and the Elide version in [`.elide-version`](.elide-version). Rustup selects
the pinned Rust toolchain automatically. Native Image tests need GraalVM
JDK 25 with `native-image`.

```sh
make deps
make build
make check
make test
make test-native-image
```

Cargo builds Rust; Elide compiles Java and builds the JARs. `make test` covers
Rust, a revision-pinned Cargo consumer, a C consumer, and the JVM FFM binding.
`make test-native-image` runs the shared Java contract through the static binding.
Run build commands sequentially because they share output directories.

Use `make package` and `python3 tools/verify_package.py` to stage and check
packages locally. Set `ELIDE` to choose the build executable, `JAVA_HOME` for
JVM tools, or `BEMO_TEST_JAVA` to test with another JVM. Windows builds also
need NASM or `AWS_LC_SYS_PREBUILT_NASM=1`; OpenSSL on `PATH` enables additional
TLS interoperability checks.

Source and documentation:

- [`crates/bemo`](crates/bemo): Rust core.
- [`crates/bemo-ffi`](crates/bemo-ffi) and [`include`](include): C interface.
- [`packages`](packages): Java API, FFM, Native Image, and Netty adapters.
- [Architecture](docs/architecture.md) and [I/O ownership](docs/transport-io.md): how the pieces fit together.
- [Measurement](docs/measurement.md) and [native safety](docs/native-safety.md): benchmarks, coverage, sanitizers, and fuzzing.

## About

Bemo is the main transport for [Elide](https://github.com/elide-dev/elide).
The name comes from the [minibuses](https://en.wikipedia.org/wiki/Share_taxi#Indonesia)
that carry people around Indonesia, including [Bali](https://github.com/elide-dev/bali).

Apache-2.0.
