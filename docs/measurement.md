# Tests, coverage, and continuous performance

Cargo owns Rust builds and Nextest. Elide resolves JVM dependencies and compiles
all production/test/benchmark Java. JVM contracts deliberately execute on stock
Java, without Elide runtime classes on their classpath. Native Image contracts
exercise the other binding independently.

## Test and coverage reports

Install the tool versions recorded in `tools/versions.json` (CI installs pinned
binaries): `cargo-nextest`, `cargo-llvm-cov`, and, for performance work,
`cargo-codspeed`. Add `llvm-tools-preview` to the pinned Rust toolchain for
coverage. `make deps` resolves JaCoCo through Elide; no separate Maven build is
introduced.

| Command | Reports |
| --- | --- |
| `make test-rust` | `build/reports/tests/rust/ci/junit.xml` |
| `ELIDE_TRANSPORT_TEST_BACKEND=epoll cargo nextest run --workspace --lib --tests --locked --profile epoll` | `build/reports/tests/rust/epoll/junit.xml` |
| `make test-jvm` | `build/reports/tests/jvm/junit.xml`, individual `TEST-*.xml`, logs |
| `make test-native-image` | `build/reports/tests/native-image/junit.xml`, individual XML and logs |
| `make coverage-rust` | Rust test XML and `build/reports/coverage/rust/lcov.info` |
| `make coverage-jvm` | JVM test XML and `build/reports/coverage/jvm/jacoco.xml`, HTML, execution data |
| `make coverage` | Both coverage suites, sequentially |

Nextest reports each Rust test separately, with no retries and a two-minute
per-test termination budget. Doctests still run with Cargo because Nextest does
not execute them. Benchmark correctness runs separately (`make bench-smoke`),
so switching away from `cargo test --all-targets` does not silently remove it.

The existing Java tests are process-level contracts with `main` entry points,
not JUnit methods. `tools/reports.py` emits one test case per contract, including
elapsed time, captured output, exceptions, nonzero exits, and timeouts. It runs
the remaining contracts after a failure, then exits unsuccessfully. This retains
the current isolation and stock-JVM check; it does not claim to use Elide's
JUnit discovery engine. Native Image's combined transport contract remains one
case. Compilation failures fail the job; they are not invented passing test cases.

JaCoCo instruments only Bemo packages. Its report includes API, FFM, and Netty
classes, not dependencies or test harnesses. Native Image adapters are verified
by C API tests but are **not** included in JVM coverage: JaCoCo cannot instrument
them while running a native executable. Rust LCOV excludes integration-test and
benchmark sources. Coverage and tests remain separate upload types.

CI archives reports even on failures, on every PR and main verification. Uploads are enabled by default after connecting `elide-dev/bemo` in Codecov.
Main-branch workflows forward `CODECOV_TOKEN`; PR and merge-queue workflows use
Codecov OIDC trust and receive no secrets. Set repository variable
`CODECOV_ENABLED=false` to disable external uploads while retaining artifacts. Uploads use explicit file paths, `rust`/`jvm` flags, and disabled file
search. Codecov statuses start informational; establish measured coverage
baselines before setting numeric merge requirements. The build and test jobs
still fail on real failures. Reports are available as artifacts even when
Codecov is disabled. `codecov.yml` maps JVM package-relative sources back to
Elide's separate source roots.

Do not run build/test/coverage targets concurrently in one checkout: they share
Java class output directories. Coverage resets its own execution data before
each run. Native and JVM XML have separate directories.

## Performance measurement

`check.bench.yml` is called by the reusable `on.verify.yml` flow for PRs, main
pushes, scheduled checks, and manual dispatch.
It follows Bali's and Komodo's split between CPU simulation and wall time.
Benchmark execution participates in the same Ready gate as correctness checks.
Main push runs retain baselines; superseded PR runs may be cancelled.

| Measurement | Continuous execution | What it covers |
| --- | --- | --- |
| CodSpeed CPU simulation | Every run, hosted Linux | Rust buffers, handles, complete/fragmented HTTP/1 request parsing, response encoding |
| Native/JVM loopback RPS and RSS | Every run, hosted Linux | Native Netty channels, HTTP/1 codec, plaintext/TLS, identity/gzip, 1 KiB/64 KiB bodies |
| CodSpeed wall time | Trusted runs on `linux-amd64-bench` | TLS 1.2/1.3 handshakes and records; all eight end-to-end workloads |

The Rust benches use Criterion through CodSpeed's compatibility crate, so local
`cargo bench` remains available. HTTP parsing includes receive-buffer allocation,
fragmentation and ownership; response encoding includes native allocation and
cached Date headers. These are operation costs, not isolated tokenizer-only
numbers. TLS full-handshake benchmarks disable client session resumption;
record benchmarks warm an established session and test both directions at
64 B, 4 KiB, and 64 KiB. TLS randomness/crypto and network scheduling belong in
wall-time measurement, not the deterministic simulation set. Benchmarks assert
successful parsing, encoding, handshakes, and byte-exact record delivery.

The primary macro workload runs a V2 native HTTP server compiled as an optimized
Native Image (`-O3`) and statically linked to Bemo through its C bindings. Rust
owns request parsing, response encoding, socket I/O, and Rustls/aws-lc-rs TLS.
The comparator is a stock OpenJDK Netty HTTP server with epoll on Linux or kqueue
on macOS and JDK TLS. Explicit NIO comparison is also available. Neither server
falls back silently when its requested backend is unavailable.

Both servers run in separate processes from the same stock OpenJDK Netty load
generator, with one server I/O thread and one client I/O thread. Four persistent
clients issue one outstanding request each. Payload, response headers, connection
count, 256 MiB heaps, warmup and measurement rounds match. Launch order rotates
across samples. The client always uses the selected platform-native Netty transport,
JDK TLS 1.3 with certificate and hostname verification, Netty HTTP response codecs,
and byte-exact payload validation. Native HTTP currently implements server-side
parsing/encoding; the common client is deliberately held constant.

`--runtime jvm` runs the same V2 HTTP server through FFM as a runtime control.
`--http-provider netty --runtime jvm` selects the legacy Netty-codec transport
control, and `--tls-provider jdk` holds TLS constant in that control. The defaults
use native HTTP and Rustls/aws-lc-rs for Bemo. Results record server runtime,
binding, HTTP provider, TLS provider, actual backend, and load-generator stack.
Full-stack comparisons are labeled as such, including plain HTTP comparisons.

Gzip is application compression, not a native Bemo codec: the native HTTP server
uses `java.util.zip.GZIPOutputStream` per response; the comparator uses Netty's
`HttpContentCompressor`. Both clients use the same decompressor and verify
compression negotiation. Identity workloads isolate the native HTTP/TLS data
plane. Bodies repeat a fixed JSON pattern, so gzip results describe compressible
application data, not incompressible data.

`make bench-prepare` fingerprints source inputs, compiled classes, the native
library, and the optimized Native Image server; measurement refuses stale
preparations. Generated `target/` directories
and Python caches are excluded from source fingerprints. Wall-time CI sets
`CRITERION_HOME` explicitly and uploads Criterion reports alongside transport
measurements. A stale preparation writes `manifest-drift.json` with the changed
fingerprint categories, changed source file names, and expected/actual hashes
before failing. Cargo.lock remains part of the fingerprint. The renamed package
entries use Cargo's canonical ordering, and the wall-time workflow prepares the
JVM workload after building CodSpeed's Rust benchmarks.

Each sample starts fresh server and client processes with fixed 256 MiB heaps,
warms 5,000 rounds, then
times 25,000 rounds (100,000 completed requests). Setup, TLS handshakes, warmup,
and teardown are outside the requests/sec timer. Three samples produce a median
RPS, and all samples are retained. Per-response p50/p99 latency records enqueue
through validated reply completion; process CPU is measured over the request
interval and records server and client costs separately as well as their sum.
This is a closed-loop benchmark: its
latency percentiles do not correct coordinated omission or measure externally
offered load. RSS sums Linux `VmRSS` before/after measurement
and each process's `VmHWM` over its **entire lifetime**. The sum of high-water marks
is not a simultaneous peak. It includes both runtimes, native allocations,
server, client, codecs, and warmup. It is not server-only RSS, a
live-allocation counter, or heap size. macOS local runs explicitly emit null RSS;
Linux CI requires actual RSS readings. CodSpeed simulation does not measure RSS.

CodSpeed tracks Bemo and stock Netty native transports as separate fixed-work
commands. Each command runs one transport; combining them would hide which
transport changed. The generic command integration times the complete macro
process, including JVM startup and warmup; warmed RPS is a separate JSON metric.
Do not label its wall-time number as steady-state requests/sec. The Rust TLS
integration measures benchmark regions directly. Dedicated runner jobs never
execute fork PR code. The variable is intentionally required, as in Komodo,
to avoid jobs queuing forever against a runner label that is not provisioned.
CodSpeed uses the existing v5.2.1 action/runner convention and OIDC authentication;
the repository must be connected in CodSpeed for uploaded comparisons to appear.

Local commands:

```sh
make bench-smoke             # all Rust benchmark correctness paths, including TLS
make bench                  # local Criterion measurements
make bench-prepare          # Cargo release library + Elide JVM compilation + optimized Native Image
make bench-transport        # full loopback measurement matrix
python3 tools/bench.py run --case tls-gzip-65536 --samples 1
python3 tools/bench.py run --clients 32 --transports bemo,netty-native,nio
python3 tools/bench.py run --transports bemo --runtime jvm
python3 tools/bench.py run --http-provider netty --runtime jvm --tls-provider jdk
# Use a specific stock JDK for local measurements:
BEMO_BENCH_JAVA=/path/to/jdk/bin/java make bench-transport
# Fast functionality check, not a performance baseline:
python3 tools/bench.py run --rounds 20 --warmup 5 --samples 1
```

`build/reports/benchmarks/` contains per-sample JSON/logs and summary JSON; CI
also writes a table to its job summary and retains artifacts for 30 days.
`summary-*-comparison.json` records Bemo/comparator throughput and p99 ratios,
including each sample's throughput ratio. Ratios are measurements of this
workload, not universal superiority claims or statistical confidence bounds. The
hosted run compares against the most recent available main artifact and emits
advisory warnings for >25% median RPS loss or >20% peak RSS growth. Workload,
architecture, and Java-version changes invalidate the comparison. These initial
thresholds identify large changes; they are not a measured noise model. Use the
provisioned wall-time runner and repeated samples before tightening thresholds
or treating an RSS/RPS warning as a blocker. New cases establish a baseline on
main before any comparison can be made.

Next extensions should add incompressible payloads, sustained-load memory slopes,
HTTP/2 multiplexing and open-loop offered-load measurement. Concurrency
sweeps and explicit Bemo backend selection are available locally; the default
CI matrix uses four clients and AUTO. The initial matrix does not imply
coverage of those performance dimensions.

References: [Nextest JUnit](https://nexte.st/docs/machine-readable/junit/),
[CodSpeed Criterion integration](https://codspeed.io/docs/benchmarks/rust/criterion),
[JaCoCo](https://www.jacoco.org/jacoco/trunk/doc/).
