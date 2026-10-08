# Spring Boot, Micronaut, and Ktor examples

`examples/spring-boot`, `examples/micronaut`, and `examples/ktor` each contain
an `elide.pkl`, `pom.xml`, Gradle build, and pinned Gradle wrapper beside shared
application sources (Java for Spring/Micronaut, Kotlin for Ktor).
All expose `GET /plaintext`, returning `Hello, World!` with `Content-Type:
text/plain`, on loopback port 8080. All also serve the same framework routes
over HTTPS on loopback port 8443.

All applications run on the JVM and as GraalVM Native Images. Bemo is enabled
by default. JVM applications use FFM and the platform shared library; native
executables use the C API and a statically linked Bemo archive. The shared
`BemoRuntime` factory selects the binding without changing the HTTP handler.
The native Netty baseline is selected at runtime, including in the same native
executable, with `-Dbemo.enabled=false`.

Spring Boot WebFlux retains Reactor Netty HTTP routing and codecs. Its
`ReactorResourceFactory` selects Bemo's loops or stock loops at runtime;
Spring AOT therefore does not freeze the benchmark's transport choice.
Micronaut retains its own HTTP server and codecs, with
`BemoEventLoopGroupFactory` supplying Bemo's I/O handler and TCP channels. Its
acceptor and worker groups share one binding for socket ownership transfer.
The compression workloads additionally invoke Bemo's reusable native gzip
encoder, and the HTTPS listener uses `NativeSslContext` over Rustls/aws-lc-rs.
The frameworks retain HTTP parsing, routing, and response encoding.

Ktor uses its Netty engine's `configureBootstrap` hook to install Bemo acceptor
and worker groups with one shared binding and a `NativeServerSocketChannel`
factory. `shareWorkGroup` keeps coroutine calls on the worker loops. The HTTPS
server uses the same Ktor routes and inserts the shared TLS context through
`channelPipelineConfig`. Each engine owns and stops its groups before the TLS
workload is released. HTTP/2 is disabled for parity with the existing examples.
Ktor and Kotlin versions are pinned in `tools/versions.json`; the Gradle Kotlin
plugin and Maven Kotlin compiler use the same Kotlin version as Elide.
See [Ktor's Netty configuration](https://api.ktor.io/ktor-server-netty/io.ktor.server.netty/-netty-application-engine/-configuration/index.html).

## Native Netty comparison baseline

`-Dbemo.enabled=false` requires Netty epoll on Linux or kqueue on macOS and
`SslProvider.OPENSSL_REFCNT` using pinned `netty-tcnative-boringssl-static`.
Missing native libraries fail startup; the examples never fall back to NIO
or JDK TLS. The framework benchmark runner checks the actual server channel
class and tcnative context diagnostic before warmup, and records the driver,
channel class, and native TLS implementation with each sample. Correctness
checks also require the example's owned TLS context reference to be released;
Micronaut releases its separately retained reference through its context holder.

Elide, Maven, and Gradle include the native transport classifiers. Elide 1.5.4
omits resolved classifiers from its runtime classpath, so preparation stages
`build/examples/netty-native.jar` with the current platform's pinned JNI
libraries. Native Image embeds these resources and uses the shared JNI metadata
in `examples/shared/native`; Bemo remains statically linked. The metadata was
recorded with the GraalVM agent against Netty 4.2.18 and tcnative 2.0.84, with
Linux epoll JNI types and both platforms' TCP constructors included explicitly.
Refresh and requalify it when changing these dependencies.

The comparison's gzip helper remains the reusable JDK Deflater at the same
level as Bemo. Historical framework reports used NIO/JDK TLS; their recorded
numbers do not describe this corrected native baseline. No replacement timing
results are available yet.

## Workload endpoints

| Endpoint | Benchmark URL | Decoded body | Compression |
| --- | --- | --- | --- |
| `/plaintext` | `http://127.0.0.1:8080/plaintext` | 13 bytes | None |
| `/payload` | `http://127.0.0.1:8080/payload` | 128 KiB | None |
| `/compression` | `http://127.0.0.1:8080/compression` | 128 KiB | Negotiated gzip |
| `/tls` | `https://127.0.0.1:8443/tls` | 128 KiB | None |
| `/tls-compression` | `https://127.0.0.1:8443/tls-compression` | 128 KiB | Negotiated gzip |

All large endpoints return the same deterministic ASCII payload. Compression
requires an acceptable `Accept-Encoding: gzip` or wildcard; absent headers or
`gzip;q=0` select identity. Compressed responses set `Content-Encoding: gzip`,
`Vary: Accept-Encoding`, and the compressed wire length. The TLS endpoints
reject requests on the HTTP listener with status 426.

The gzip operation runs anew for every request. Each Netty worker reuses one
compressor and, in native mode, one frozen input buffer; compressed responses
are not cached. Native output is copied into the framework's byte-array response
before its native handle is released. Netty thread cleanup releases the owner,
input, and encoder; Micronaut also schedules explicit cleanup on its workers
before destroying its event-loop registry. Foreign executor threads use scoped
compressors rather than retain thread-local native state.

Both modes default to gzip level **1**. `-Dbemo.gzip.level=6`, for example,
changes the level for both native zlib-rs and the reusable JDK Deflater baseline
(accepted levels: 0–9). Provider
headers (`X-Compression-Provider`, `X-TLS-Provider`) identify the selected
implementation. Stock mode initializes neither Bemo gzip nor Rustls.

HTTPS uses a **public test certificate and key** copied from the repository's
localhost fixtures and packaged with every builder. This identity is for local
examples and benchmarks. Its certificate covers `localhost` and `127.0.0.1`;
the smoke tests verify it rather than disabling verification. Example requests:

```sh
curl --compressed http://127.0.0.1:8080/compression
curl --cacert examples/shared/src/main/resources/benchmark-tls/localhost-cert.pem \
  https://127.0.0.1:8443/tls
curl --compressed \
  --cacert examples/shared/src/main/resources/benchmark-tls/localhost-cert.pem \
  https://127.0.0.1:8443/tls-compression
```

Native/JVM system properties `bemo.tls.port` and `bemo.tls.enabled` change the
HTTPS port or disable that listener. Maven's `exec:exec` accepts these and
`bemo.gzip.level` as `-D` properties; Gradle's `run` and `nativeRun` accept
them as `-P` properties. They are runtime choices, including in native images.

## Prerequisites and staging

Use the repository's pinned Elide toolchain, Python 3.11+, Rust, and a JDK 22+.
Set `JAVA_HOME` to that JDK. Native builds also need GraalVM `native-image`
compatible with the pinned GraalVM SDK, and a C toolchain. Maven is needed only
for the Maven path; the Gradle wrapper downloads the pinned distribution.

From the repository root:

```sh
make examples-prepare
```

This builds Bemo with Cargo and Elide, then stages all Java bindings, the
release-mode shared library, static archive, and headers beneath
`build/examples`. Staging neither publishes packages nor writes Bemo into your
global Maven cache. Maven and Gradle build only the consumer applications.
The Gradle Bemo dependencies are changing modules so a newly staged checkout
is used even when its version has not changed.

## JVM builds

Use any example directory in these commands.

### Elide

```sh
make examples-build # from the repository root
cd examples/spring-boot # or examples/micronaut or examples/ktor
elide run
elide -f STOCK run # stock baseline; run separately
```

After staging, `elide install` and `elide build` work inside any project.
Elide 1.5.4 uses an external JVM compiler for Micronaut's annotation processors.
Its manifests name configuration files directly because that Elide version
does not copy application resources into the compiled output.

### Maven

```sh
cd examples/spring-boot # or examples/micronaut or examples/ktor
mvn -Dmaven.repo.local=../../build/examples/m2 package
mvn -Dmaven.repo.local=../../build/examples/m2 exec:exec
mvn -Dmaven.repo.local=../../build/examples/m2 -Dbemo.enabled=false exec:exec
```

### Gradle

```sh
cd examples/spring-boot # or examples/micronaut or examples/ktor
./gradlew build
./gradlew run
./gradlew -Pbemo.enabled=false run
```

Gradle also provides the application plugin's `installDist` task for a JVM
application distribution. Use `gradlew.bat` on Windows.

For any launcher, check the response from another terminal:

```sh
curl -i http://127.0.0.1:8080/plaintext
```

Stop with Ctrl-C. Bemo mode prints its binding (`FFM` or `CAPI`) at startup.
Stock mode does not initialize Bemo.

## Native Image builds

All three build paths share the native pipeline in `tools/examples.py`. It
runs Spring AOT processing and compiles the generated initializers with Elide;
Micronaut supplies its generated bean definitions and GraalVM metadata during
normal annotation processing. All then link the staged optimized Bemo archive
with Native Image. Netty is initialized at runtime so logging state is not
captured in the image heap.

Build all examples with Elide from the repository root:

```sh
make examples-native
```

Or build one inside its project directory:

```sh
elide run native
mvn -Dmaven.repo.local=../../build/examples/m2 -Pnative package
./gradlew nativeCompile
```

Executables are written to `build/examples/native/<builder>/<project>` relative
to the repository root, where `<builder>` is `elide`, `maven`, or `gradle`, and
`<project>` is `spring-boot` or `micronaut`. Windows adds `.exe`.

From the repository root:

```sh
build/examples/native/elide/spring-boot
build/examples/native/elide/spring-boot -Dbemo.enabled=false
build/examples/native/elide/micronaut
build/examples/native/elide/micronaut -Dbemo.enabled=false
```

Run these separately; all default to port 8080. Pass `-Dserver.port=8081` to
Spring or `-Dmicronaut.server.port=8081` to Micronaut to change the port.
Gradle also offers `nativeRun`, with `-Pbemo.enabled=false` for stock transport.

Keep each native output directory's support libraries beside its executable
when moving it. No external Bemo shared library is needed by native executables;
they use static C linkage. The JVM path loads its native library from a staged
classifier JAR without an explicit library path.

Native builds default to `-O2`. `BEMO_NATIVE_JOBS` controls build parallelism;
the default is at most eight threads. The helper's `--native-opt` option selects
another optimization level, for example:

```sh
python3 tools/examples.py native --project micronaut --native-opt 3
```

## Verification

```sh
make test-examples
make test-examples-native
python3 tools/examples.py test --builder maven
python3 tools/examples.py test --builder maven --native
python3 tools/examples.py test --builder gradle
python3 tools/examples.py test --builder gradle --native
```

Each command builds and starts all frameworks with Bemo and stock Netty,
checks every endpoint's exact decoded response and content type, verifies gzip
members and CRCs, tests negotiation and TLS requirements, validates TLS 1.2 and
1.3 using the fixture trust anchor, and exercises concurrent requests and
connection reuse. It then verifies binding/provider and native reclamation
diagnostics and stops each server. Logs are
saved under `build/examples`. Test existing executables without rebuilding:

```sh
python3 tools/examples.py smoke --builder gradle --native
```

## Benchmarking

The handlers exclude databases, JSON serialization, and blocking application
work. Micronaut runs the handler on the I/O thread.
Defaults use two workers. Micronaut has one separate acceptor thread; the Spring
Bemo example shares its worker group with its acceptor. Compare each framework's
Bemo and stock modes under the same JVM or native execution mode.

The staged Rust artifacts use Cargo's release profile. Hold the JDK/GraalVM,
Native Image optimization level, worker and acceptor topology, native backend,
concurrency, warmup, and HTTP/TLS/compression settings fixed before measuring.
AUTO chooses Bemo's available native backend and may fall back from io_uring.
Correctness smoke tests do not report performance numbers.

On Linux, prepare JVM classes and native executables, then run the validating
framework benchmark separately from builds and tests:

```sh
make examples-prepare
python3 tools/examples.py test
python3 tools/examples.py test --native --skip-build --native-opt 3
python3 tools/bench_frameworks.py --server-cpus 12-17 --client-cpus 18-21 \
  --output build/reports/framework-initial
```

Choose disjoint server and client CPU sets for your host's physical topology.
The runner uses four wrk threads, 64 persistent connections, a 256 MiB initial
and maximum Java heap, two server workers, and three fresh processes per case.
Each process receives 20 seconds of warmup followed by 20 seconds of measured
load. Transport order alternates between repetitions; framework/runtime pairs
rotate. The Lua client checks every completed response's status and body;
startup also verifies content type and connection reuse. Any invalid response,
socket error, or timeout aborts the run. Warmup, duration, sample count, client
connections, and client thread count are command-line options; heap and worker
settings are fixed by this runner.

Results include individual samples, wrk/server logs, medians and ranges, server
CPU and Linux RSS, toolchain/host information, and artifact/harness hashes.
An output directory must be new. Startup timing is time to the first verified
HTTP response with polling, rather than a precise process initialization timer.
RSS includes native allocations; the heap setting is not a total memory cap.
The load generator runs on the same host, and its Lua validation adds client
work. This is a closed-loop plaintext measurement at one concurrency, not a
capacity or open-loop latency test.

Use `--probe --output build/reports/framework-probes` in a separate invocation
to trace backend syscalls without measuring performance under strace. AUTO
fallback diagnostics are also preserved in each measured process's logs.
The runner requires Python 3.11+, `wrk`, `taskset`, and Linux `/proc`; probes
additionally require `strace`. Gzip validation uses LuaJIT FFI and the system
zlib shared library. It does not build artifacts or modify host tuning.

Select one workload per output directory with `--workload plaintext`,
`payload`, `compression`, `tls`, or `tls-compression`. The large-body identity,
gzip, and TLS cases all use 128 KiB. HTTPS readiness verifies the fixture
certificate; wrk uses OpenSSL HTTPS for load generation. Its Lua response hook
inflates every gzip reply, verifies its CRC and exact decoded bytes, rejects
trailing wire data, and checks compression/TLS provider headers. This decoding
adds client work and must be considered when increasing concurrency or throughput.

For example, after building the applications and native images:

```sh
python3 tools/bench_frameworks.py --server-cpus 12-17 --client-cpus 18-21 \
  --workload compression --gzip-level 1 --output build/reports/framework-gzip
python3 tools/bench_frameworks.py --server-cpus 12-17 --client-cpus 18-21 \
  --workload tls --output build/reports/framework-tls
python3 tools/bench_frameworks.py --server-cpus 12-17 --client-cpus 18-21 \
  --workload tls-compression --gzip-level 1 --output build/reports/framework-tls-gzip
```

Each invocation runs all frameworks, both runtimes, and both transport modes.
Non-TLS measurements disable the HTTPS listener. Gzip levels are matched across
providers; these comparisons do not repeat the earlier level-1 versus level-6
compression tuning comparison.

The [initial Unclemax results](performance/framework-initial.md) include the
three-sample JVM/native matrix, sample ranges, latency, memory, and limitations.

When changing Bemo, Netty, or GraalVM SDK pins, update the Maven properties too;
Elide and Gradle read the repository version files. The Spring and Micronaut
versions are pinned in each build definition. The direct Spring AOT invocation
is tied to the pinned Spring Boot version; verify it when upgrading Spring.

The [native-baseline cycle](performance/native-baseline.md) covers Spring Boot,
Micronaut and Ktor across all five workloads in both runtimes, against native
epoll and tcnative/BoringSSL. The [matched-level rerun](performance/unclemax-matched.md)
retains the archived NIO/JDK TLS measurements. The client records the minimum and maximum HTTP body
size before decoding; these sizes exclude HTTP headers and TLS framing.

TLS benchmark clients now use the shared TLS 1.3 / AES-128-GCM policy. Run
`make bench-prepare` before TLS benchmarking to build the Linux OpenSSL client
policy shim; it requires OpenSSL development headers. The benchmark runner
checks negotiation during readiness and records the policy and shim hash.

## Ktor verification

After `make examples-prepare`, build and smoke-test Ktor through each tool:

```sh
python3 tools/examples.py test --project ktor
python3 tools/examples.py test --project ktor --builder maven
python3 tools/examples.py test --project ktor --builder gradle
```

Add `--native` to each command for a statically linked Native Image. These
checks cover Bemo and stock Netty, HTTP, gzip negotiation, TLS 1.2/1.3,
concurrent requests, keep-alive, and native workload reclamation. They do not
run benchmarks. Ktor uses `-Dserver.port` to override the HTTP listener.
