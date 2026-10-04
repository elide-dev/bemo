# Dokar

A shared Rust native transport for Elide and stock Netty, with JVM FFM and
GraalVM Native Image C bindings. Cargo builds native code; Elide resolves JVM
dependencies, compiles Java, and produces JARs.

**Status: native transport extracted; JVM adapter migration in progress.** The
Rust socket, buffer, TLS, HTTP, and workload implementation now lives here.
Existing Elide ABI 3 symbols are preserved by generated C forwarding functions.
Capability bits remain zero until both JVM binding suites are validated.

## Layout

| Path | Responsibility |
| --- | --- |
| `crates/dokar` | Runtime-independent Rust core; consumable as a Cargo Git dependency |
| `crates/dokar-ffi` | One C boundary, built as `rlib`, `staticlib`, and `cdylib` |
| `include/dokar.h` | Shared C ABI declarations |
| `packages/api` | JVM contract without runtime dependencies |
| `packages/ffm` | JDK 22+ dynamic-library adapter |
| `packages/native-image` | GraalVM C interop adapter for static linking |
| `packages/netty` | Reserved migration boundary for stock Netty 4.2 |
| `tests` | C and shared JVM binding contracts |
| `tools` | Build orchestration and artifact/consumer verification |
| `.github` | Reusable PR, push, merge-queue, check, build, and release-staging workflows |

## Develop

Install Rustup, Python 3.11+, a C toolchain, a JDK 25 build toolchain, and the
Elide version in `.elide-version`. Rustup selects `rust-toolchain.toml`.
Use a GraalVM JDK 25 with `native-image` for the static binding test.
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
`DOKAR_TEST_JAVA` to run the FFM contract with a separate stock JVM. Standard
Cargo `CARGO_TARGET_DIR` is supported. Cross-compilation is not wired into the
host binding tests or packaging commands. Build commands share output folders;
run them sequentially in a checkout.

## Consume the skeleton

After hosting this repository, pin an actual Dokar commit in the consuming
Cargo workspace:

```toml
[dependencies]
dokar = { git = "https://github.com/elide-dev/dokar", rev = "<full-commit-sha>" }
dokar-ffi = { git = "https://github.com/elide-dev/dokar", rev = "<full-commit-sha>" }
```

The proposed Maven coordinates are `dev.elide:dokar-api`, `dokar-ffm`, and
`dokar-native-image`. These are staged locally, not published. The FFM and
Native Image artifacts depend on the API artifact. GraalVM SDK dependencies
are confined to the Native Image artifact and marked `provided`.

```java
try (var transport = new FfmTransportNative(Path.of("/path/to/libdokar_ffi.so"))) {
  transport.requireCompatible();
  System.out.println(transport.capabilities()); // 0: foundation only
}
```

Run with `--enable-native-access=ALL-UNNAMED`. Load an explicit platform library
path; automatic native resource extraction is deferred until transport
migration. See [architecture](docs/architecture.md), [extraction boundaries](docs/extraction.md),
[publishing](docs/publishing.md), and [CI](docs/ci.md).

Licensed under Apache-2.0.
