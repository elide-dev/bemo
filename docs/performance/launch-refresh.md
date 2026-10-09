# Development benchmark refresh

Measured **16805b3bbc01d1ae6bd5d79376a929392a27c483** on Unclemax, October 8, 2026 (Pacific; October 9 UTC).
This revision follows the published **0.3.0** release (`0a3cc91`); these are development measurements, not release measurements.

The [earlier native baseline](native-baseline.md) remains archived with its original evidence. This is a fresh full-stack and framework matrix, not an isolated before/after optimization experiment.

Benchmark-only builds use Native Image `-march=native`, Rust `-C target-cpu=native`, and C/C++ `-march=native`. Release packaging defaults remain portable. Downloaded Netty JNI libraries retain their published CPU targets; OpenJDK selects CPU features at runtime. The source manifest identifies the small benchmark flag/provenance changes on top of the measured commit.

Toolchain: OpenJDK 25.0.2, Oracle GraalVM 25.3.4.1+1.1 (Java 25.0.4.1),
and Elide 1.5.4+20260921. Native Image reports ML-inferred PGO; no trained
profile was supplied.

## Full-stack HTTP/TLS matrix

Bemo: Native Image `-O3`, native HTTP, io_uring, Rustls/aws-lc-rs and zlib-rs. Comparator: OpenJDK, Netty HTTP, epoll and tcnative/BoringSSL. Runtime and HTTP implementations differ; gains cannot be attributed only to transport.

Matching gzip level 1 does not match compression ratios. The 128 KiB full-stack
body compresses to **1,653 B with Bemo versus 1,002 B with Netty**. Separate
[wire-size checks](data/launch-compression-sizes-20261009.json) validated three
responses per case; the Netty level 6 sizes are diagnostic and are not used in
these timing comparisons.

| Workload | Bemo req/s | Netty req/s | Change | Bemo range | Netty range |
| --- | ---: | ---: | ---: | ---: | ---: |
| plain-identity-1024 | 99,657 | 90,238 | +10.4% | 99,324–99,718 | 89,891–91,545 |
| plain-identity-65536 | 53,415 | 50,929 | +4.9% | 53,174–53,467 | 50,812–51,070 |
| plain-identity-131072 | 36,215 | 33,308 | +8.7% | 35,960–36,250 | 33,293–33,394 |
| plain-gzip-1024 | 85,909 | 62,216 | +38.1% | 81,604–86,006 | 61,165–62,511 |
| plain-gzip-65536 | 36,542 | 21,404 | +70.7% | 36,464–36,805 | 21,329–21,437 |
| plain-gzip-131072 | 22,586 | 11,072 | +104.0% | 22,538–22,623 | 11,031–11,072 |
| tls-identity-1024 | 86,036 | 77,032 | +11.7% | 85,992–86,141 | 76,930–77,305 |
| tls-identity-65536 | 26,697 | 26,869 | -0.6% | 26,325–26,730 | 26,658–27,007 |
| tls-identity-131072 | 15,497 | 15,514 | -0.1% | 15,255–15,547 | 15,037–15,566 |
| tls-gzip-1024 | 75,911 | 56,455 | +34.5% | 74,302–76,418 | 56,445–56,740 |
| tls-gzip-65536 | 41,860 | 20,468 | +104.5% | 41,310–42,050 | 20,449–20,639 |
| tls-gzip-131072 | 25,885 | 10,799 | +139.7% | 25,860–25,990 | 10,778–10,804 |

Each of the 72 samples completes 100,000 measured requests: **7,200,000** total. Four persistent clients, one outstanding request each, warm up 5,000 rounds and measure 25,000 rounds per client. Server/client heaps are 256 MiB, with one I/O thread each. Both servers use the same external OpenJDK Netty NIO/JDK TLS client. Server affinity is CPUs 12–17; client affinity is 18–21.

| Workload | Bemo p99 µs | Netty p99 µs | Bemo server CPU µs/req | Netty server CPU µs/req | Bemo combined RSS MiB | Netty combined RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| plain-identity-1024 | 40.4 | 44.5 | 6.8 | 8.8 | 275.3 | 470.4 |
| plain-identity-65536 | 76.4 | 81.4 | 13.1 | 15.5 | 300.7 | 511.3 |
| plain-identity-131072 | 116.4 | 125.2 | 20.1 | 24.7 | 285.0 | 513.2 |
| plain-gzip-1024 | 50.9 | 66.2 | 7.8 | 14.7 | 293.0 | 537.1 |
| plain-gzip-65536 | 112.3 | 189.7 | 14.5 | 40.4 | 300.9 | 554.0 |
| plain-gzip-131072 | 184.0 | 365.2 | 22.4 | 80.1 | 302.9 | 554.1 |
| tls-identity-1024 | 55.9 | 54.4 | 7.7 | 10.6 | 309.1 | 553.2 |
| tls-identity-65536 | 151.1 | 151.5 | 19.9 | 22.5 | 307.0 | 569.4 |
| tls-identity-131072 | 266.7 | 263.5 | 30.8 | 36.7 | 309.0 | 569.7 |
| tls-gzip-1024 | 68.7 | 72.9 | 8.5 | 15.6 | 306.4 | 577.5 |
| tls-gzip-65536 | 103.2 | 197.8 | 15.3 | 41.8 | 317.6 | 582.0 |
| tls-gzip-131072 | 168.4 | 375.3 | 23.4 | 81.7 | 323.5 | 583.3 |

RSS above is the maximum sample sum of server/client lifetime high-water marks, including startup and warmup; it is not simultaneous peak memory, server-only memory, or heap size.

## Framework matrix

Runtime and framework HTTP codecs stay fixed within each pair. Spring Boot WebFlux, Micronaut and Ktor retain their Java HTTP parsing/routing/encoding. Transport, TLS and compression providers are selected explicitly. Gzip compares reusable native zlib-rs with the reusable **JDK Deflater** helper, not zlib-ng.

| Framework | Runtime | Endpoint | Bemo req/s | Native Netty req/s | Change | Bemo range | Netty range |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| spring-boot | jvm | plaintext | 158,532 | 155,670 | +1.8% | 155,893–162,136 | 155,413–156,753 |
| spring-boot | native | plaintext | 121,009 | 117,984 | +2.6% | 120,345–121,014 | 117,212–118,872 |
| micronaut | jvm | plaintext | 277,643 | 262,928 | +5.6% | 271,526–277,826 | 258,216–264,858 |
| micronaut | native | plaintext | 204,741 | 193,177 | +6.0% | 199,243–207,728 | 192,763–194,018 |
| ktor | jvm | plaintext | 246,123 | 246,542 | -0.2% | 245,527–249,660 | 242,857–248,995 |
| ktor | native | plaintext | 206,529 | 199,102 | +3.7% | 206,374–206,547 | 198,610–199,446 |
| spring-boot | jvm | payload | 57,675 | 66,839 | -13.7% | 56,826–57,893 | 65,425–67,009 |
| spring-boot | native | payload | 51,237 | 51,660 | -0.8% | 50,441–52,042 | 51,554–52,534 |
| micronaut | jvm | payload | 70,378 | 84,551 | -16.8% | 69,220–71,058 | 82,777–84,628 |
| micronaut | native | payload | 61,264 | 68,219 | -10.2% | 61,003–64,612 | 67,313–68,566 |
| ktor | jvm | payload | 68,894 | 79,021 | -12.8% | 68,009–69,203 | 78,570–79,341 |
| ktor | native | payload | 65,898 | 57,295 | +15.0% | 65,408–66,769 | 52,667–58,347 |
| spring-boot | jvm | compression | 56,159 | 24,138 | +132.7% | 55,954–57,343 | 24,021–24,166 |
| spring-boot | native | compression | 48,262 | 9,115 | +429.5% | 48,032–48,274 | 9,112–9,117 |
| micronaut | jvm | compression | 81,768 | 27,140 | +201.3% | 80,913–81,993 | 27,050–27,159 |
| micronaut | native | compression | 70,473 | 9,631 | +631.7% | 70,025–70,698 | 9,627–9,645 |
| ktor | jvm | compression | 79,226 | 27,223 | +191.0% | 78,861–81,666 | 27,170–27,412 |
| ktor | native | compression | 73,532 | 9,742 | +654.8% | 73,336–73,582 | 9,731–9,744 |
| spring-boot | jvm | tls | 40,391 | 40,902 | -1.3% | 40,276–40,586 | 40,652–41,309 |
| spring-boot | native | tls | 34,250 | 33,435 | +2.4% | 34,242–34,420 | 33,196–33,671 |
| micronaut | jvm | tls | 47,382 | 53,209 | -11.0% | 46,902–47,896 | 52,717–54,133 |
| micronaut | native | tls | 39,651 | 39,766 | -0.3% | 39,575–39,978 | 39,703–39,806 |
| ktor | jvm | tls | 52,136 | 53,652 | -2.8% | 51,912–53,481 | 53,409–54,216 |
| ktor | native | tls | 46,849 | 45,993 | +1.9% | 46,736–46,978 | 45,207–46,346 |
| spring-boot | jvm | tls-compression | 50,899 | 23,227 | +119.1% | 50,795–52,154 | 23,168–23,298 |
| spring-boot | native | tls-compression | 42,629 | 8,922 | +377.8% | 42,536–42,797 | 8,898–8,943 |
| micronaut | jvm | tls-compression | 73,052 | 26,488 | +175.8% | 72,725–73,164 | 26,431–26,578 |
| micronaut | native | tls-compression | 59,314 | 9,478 | +525.8% | 59,138–59,812 | 9,469–9,489 |
| ktor | jvm | tls-compression | 77,259 | 26,729 | +189.0% | 76,866–77,280 | 26,616–26,814 |
| ktor | native | tls-compression | 68,283 | 9,648 | +607.8% | 68,147–68,735 | 9,639–9,649 |

All **180 samples** completed **282,785,966 responses**. Each fresh process receives 20 seconds of warmup and 20 seconds of measured load: four wrk threads, 64 persistent connections, two server workers, and 256 MiB JVM heaps. Launch order alternates and framework/runtime pairs rotate. Native Images use `-O3`, host-native CPU targeting (`-march=native`), and no trained PGO. Raw samples retain latency, server CPU/RSS, startup time, provider evidence, and wire body sizes.

## Qualifications

These are three-sample closed-loop loopback measurements on a shared Linux Threadripper PRO 9965WX host. Ranges are observed sample minima/maxima, not confidence intervals. Databases, serialization and blocking application work are excluded. The framework client validates every decoded response, adding client CPU work; gzip output sizes differ despite matching level 1: **1,580 B for Bemo versus 909 B for stock** on the 128 KiB framework body. TLS clients use TLS 1.3 / AES-128-GCM. Uncompressed regressions remain visible in the tables and README.

All stock framework samples must identify epoll server channels and tcnative/BoringSSL for TLS. Chart validation rejects incomplete matrices, backend fallbacks, mismatched artifacts/settings, and invalid responses. The basic harness rejects source/artifact drift after preparation. See [measurement](../measurement.md) for ownership and client/server controls.

## Evidence and reproduction

Separate syscall probes verified successful io_uring setup/entry for all six
Bemo framework/runtime variants and epoll waits for all six stock variants.
TLS probes verified TLS 1.3 / AES-128-GCM and stock BoringSSL. They were run
outside the timing measurements; [probe evidence](data/launch-backend-probes-20261009.json)
records the results.

All **85 Rust Criterion benchmarks** were remeasured with the native CPU flags;
[estimates and raw samples](data/launch-criterion-20261009.json) include only files
written during this run. These are unpinned Bemo-only wall-time measurements,
not a controlled before/after source experiment or a Netty/JNI comparison.
Cached Criterion baseline deltas are not used as launch claims.

The [post-cycle manifest](data/launch-post-cycle-20261009.json) matches every
prepared source, class, library, executable, and CPU flag fingerprint.
Complete build, contract, benchmark, and probe logs are retained locally in
`build/launch/linux-native-evidence-20261009.tar.gz`; its
[archive checksum](data/launch-evidence-archive-20261009.json) records integrity.
The checked-in JSON evidence contains the raw samples needed for the charts.

- [Basic raw samples](data/launch-basic-20261009.json) and [current chart provenance](data/provenance.json).
- [All framework samples, summaries and environments](data/launch-frameworks-20261009.json).
- [Prepared artifact fingerprints](data/launch-build-20261009.json) and [AWS-LC build flags](data/launch-crypto-flags-20261009.json).
- [Source hashes](data/launch-source-20261009.json), [benchmark-only harness changes](data/launch-harness-20261009.patch), and [host/toolchain evidence](data/launch-environment-20261009.json).

Build, contract and consumer verification are recorded in [launch validation](../launch-validation.md). Source-built benchmark checks and public-release consumer checks are separate.

```sh
git worktree add --detach build/launch-benchmark 16805b3bbc01d1ae6bd5d79376a929392a27c483
git -C build/launch-benchmark apply "$PWD/docs/performance/data/launch-harness-20261009.patch"
cd build/launch-benchmark
make build
make check
make test
make test-native-image
export RUSTFLAGS='-C target-cpu=native'
export CFLAGS='-march=native'
export CXXFLAGS='-march=native'
export BEMO_BENCH_NATIVE_MARCH=native
python3 tools/examples.py prepare
python3 tools/examples.py native --native-opt 3
make bench-prepare
python3 tools/examples.py smoke
python3 tools/examples.py smoke --native
export BEMO_BENCH_GZIP_LEVEL=1
export BEMO_BENCH_SERVER_CPUS=12-17
export BEMO_BENCH_CLIENT_CPUS=18-21
python3 tools/bench.py run --backend 2
for workload in plaintext payload compression tls tls-compression; do
  python3 tools/bench_frameworks.py --workload "$workload" \
    --server-cpus 12-17 --client-cpus 18-21 \
    --output "build/reports/framework-$workload"
done
```

Retain the complete evidence and refresh chart provenance in the documentation checkout as described in [chart generation](README.md); run `make bench-graphs` there after selecting the new data.

Use the pinned JDK/GraalVM/Elide toolchain and a stock OpenJDK load generator (`BEMO_BENCH_JAVA`). CPU sets must match the host topology and output directories must be new. Complete all builds and correctness smoke checks before timing; run syscall probes separately from timed loads; do not combine samples from different hosts or reuse older framework artifacts.
