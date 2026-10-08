# Native Netty baseline on Unclemax

This cycle compares Bemo with Netty's native **epoll** transport and
**tcnative/BoringSSL** TLS. The previous [matched-level report](unclemax-matched.md)
used JDK TLS and, for the frameworks, NIO. Its results remain archived;
changing the comparator does not establish a before/after Bemo optimization.

Bemo’s largest measured gains come from gzip: framework throughput is
**2.4–2.8×** the stock JVM path and **4.1–7.6×** the stock Native Image path.
The native output is larger (**1,580 bytes versus 909 bytes** for the framework
ASCII body at level 1). Uncompressed framework TLS in Native Image is near
parity: **+0.6%** for Spring Boot, **−2.6%** for Micronaut and **−2.7%** for Ktor.
The archived 20×-plus TLS advantage does not survive this corrected baseline.
Large uncompressed JVM framework payloads remain **15–20% behind** native Netty.

## Basic HTTP/TLS results

All 72 samples completed **7,200,000 measured requests**. Bemo has higher median
throughput in ten of twelve workloads. The exceptions are uncompressed TLS
at 64 KiB (**−15.0%**) and 128 KiB (**−12.0%**). Gzip remains the strongest
measured workload: **+102.3%** for 128 KiB plaintext and **+94.7%** for TLS+gzip.

| Workload | Bemo req/s | Netty epoll req/s | Bemo change | Bemo range | Netty range |
| --- | ---: | ---: | ---: | ---: | ---: |
| plain-identity-1024 | 99,366 | 89,051 | +11.6% | 98,599–100,555 | 89,025–90,581 |
| plain-identity-65536 | 53,708 | 51,268 | +4.8% | 52,886–53,870 | 50,333–51,392 |
| plain-identity-131072 | 36,302 | 33,331 | +8.9% | 36,206–36,466 | 33,278–33,331 |
| plain-gzip-1024 | 86,093 | 62,236 | +38.3% | 80,350–86,190 | 61,981–62,582 |
| plain-gzip-65536 | 35,771 | 21,419 | +67.0% | 35,702–35,806 | 21,339–21,461 |
| plain-gzip-131072 | 22,274 | 11,012 | +102.3% | 22,157–22,297 | 10,993–11,084 |
| tls-identity-1024 | 85,465 | 77,232 | +10.7% | 83,805–86,058 | 76,985–77,903 |
| tls-identity-65536 | 22,682 | 26,691 | -15.0% | 22,639–22,816 | 26,642–26,815 |
| tls-identity-131072 | 13,595 | 15,456 | -12.0% | 13,574–13,603 | 15,302–15,569 |
| tls-gzip-1024 | 74,030 | 56,519 | +31.0% | 73,280–74,677 | 55,587–56,773 |
| tls-gzip-65536 | 32,843 | 20,430 | +60.8% | 32,704–32,896 | 20,346–20,495 |
| tls-gzip-131072 | 21,005 | 10,790 | +94.7% | 20,913–21,089 | 10,768–10,812 |

CPU and p99 values below are sample medians. RSS is the maximum sample sum of
server/client lifetime high-water marks, in MiB.

| Workload | Bemo p99 µs | Netty p99 µs | Bemo server CPU µs/req | Netty server CPU µs/req | Bemo combined RSS MiB | Netty combined RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| plain-identity-1024 | 40.7 | 45.0 | 6.8 | 8.7 | 277.6 | 463.8 |
| plain-identity-65536 | 75.9 | 81.1 | 13.1 | 15.6 | 296.3 | 508.2 |
| plain-identity-131072 | 116.6 | 124.6 | 20.3 | 24.4 | 289.8 | 528.1 |
| plain-gzip-1024 | 49.9 | 66.0 | 7.9 | 15.3 | 292.3 | 535.7 |
| plain-gzip-65536 | 114.0 | 189.3 | 14.8 | 40.0 | 294.7 | 544.3 |
| plain-gzip-131072 | 186.5 | 366.9 | 23.0 | 80.5 | 302.8 | 555.8 |
| tls-identity-1024 | 56.6 | 54.7 | 7.5 | 10.6 | 306.9 | 542.1 |
| tls-identity-65536 | 180.0 | 152.2 | 29.0 | 22.7 | 314.8 | 559.1 |
| tls-identity-131072 | 301.4 | 266.3 | 50.8 | 36.1 | 312.8 | 549.5 |
| tls-gzip-1024 | 69.3 | 73.2 | 8.8 | 16.0 | 311.3 | 573.4 |
| tls-gzip-65536 | 124.4 | 198.5 | 15.9 | 42.0 | 320.8 | 587.6 |
| tls-gzip-131072 | 201.4 | 375.8 | 24.1 | 81.8 | 319.5 | 587.9 |

## Framework results

All **180 samples** completed **276,569,045 responses**, with zero response
validation failures, client errors or backend fallback logs. Comparisons
hold the runtime fixed and retain all five workloads.

| Framework | Runtime | HTTP 13 B | HTTP 128 KiB | Gzip 128 KiB | TLS 128 KiB | TLS+gzip 128 KiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | -0.9% | -15.1% | +141.6% | -2.6% | +126.2% |
| Spring Boot | Native Image | +3.4% | +0.1% | +474.0% | +0.6% | +420.4% |
| Micronaut | JVM | -6.8% | -20.4% | +183.4% | -12.7% | +171.1% |
| Micronaut | Native Image | +1.0% | -10.6% | +664.9% | -2.6% | +590.9% |
| Ktor | JVM | -7.0% | -20.4% | +183.2% | -7.3% | +165.8% |
| Ktor | Native Image | -3.1% | +18.5% | +305.6% | -2.7% | +259.5% |

### plaintext

| Framework | Runtime | Bemo req/s | Netty req/s | Bemo change | Bemo range | Netty range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 152,771 | 154,139 | -0.9% | 150,931–154,412 | 150,727–155,697 |
| Spring Boot | Native Image | 124,869 | 120,744 | +3.4% | 124,469–125,121 | 119,455–121,149 |
| Micronaut | JVM | 247,336 | 265,381 | -6.8% | 243,689–247,873 | 265,144–265,998 |
| Micronaut | Native Image | 197,963 | 196,063 | +1.0% | 197,465–199,068 | 194,921–200,704 |
| Ktor | JVM | 226,569 | 243,626 | -7.0% | 223,124–227,416 | 240,032–246,712 |
| Ktor | Native Image | 192,602 | 198,830 | -3.1% | 192,435–194,632 | 198,037–199,517 |

### payload

| Framework | Runtime | Bemo req/s | Netty req/s | Bemo change | Bemo range | Netty range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 57,071 | 67,185 | -15.1% | 56,325–57,243 | 66,206–69,144 |
| Spring Boot | Native Image | 51,321 | 51,255 | +0.1% | 51,223–51,609 | 50,843–52,997 |
| Micronaut | JVM | 66,658 | 83,691 | -20.4% | 65,932–67,775 | 83,022–84,720 |
| Micronaut | Native Image | 61,530 | 68,854 | -10.6% | 60,694–61,785 | 67,083–70,023 |
| Ktor | JVM | 62,691 | 78,720 | -20.4% | 62,438–62,881 | 77,521–79,232 |
| Ktor | Native Image | 59,190 | 49,943 | +18.5% | 59,014–59,232 | 48,855–52,863 |

### compression

| Framework | Runtime | Bemo req/s | Netty req/s | Bemo change | Bemo range | Netty range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 58,297 | 24,132 | +141.6% | 57,942–58,778 | 24,025–24,159 |
| Spring Boot | Native Image | 48,930 | 8,525 | +474.0% | 48,825–49,102 | 8,517–8,529 |
| Micronaut | JVM | 76,755 | 27,086 | +183.4% | 76,037–77,408 | 27,067–27,128 |
| Micronaut | Native Image | 68,911 | 9,009 | +664.9% | 68,686–69,561 | 8,992–9,013 |
| Ktor | JVM | 77,033 | 27,202 | +183.2% | 76,618–77,385 | 27,157–27,405 |
| Ktor | Native Image | 70,946 | 17,492 | +305.6% | 70,594–71,194 | 17,442–17,821 |

### tls

| Framework | Runtime | Bemo req/s | Netty req/s | Bemo change | Bemo range | Netty range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 40,301 | 41,362 | -2.6% | 40,142–40,654 | 40,927–41,555 |
| Spring Boot | Native Image | 33,516 | 33,304 | +0.6% | 33,276–33,606 | 33,251–33,392 |
| Micronaut | JVM | 45,929 | 52,620 | -12.7% | 45,776–46,078 | 52,586–53,201 |
| Micronaut | Native Image | 38,965 | 40,011 | -2.6% | 38,788–39,677 | 40,007–40,235 |
| Ktor | JVM | 48,952 | 52,825 | -7.3% | 48,712–49,089 | 52,148–53,314 |
| Ktor | Native Image | 43,639 | 44,852 | -2.7% | 43,191–43,882 | 44,189–44,995 |

### tls-compression

| Framework | Runtime | Bemo req/s | Netty req/s | Bemo change | Bemo range | Netty range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 52,391 | 23,164 | +126.2% | 52,352–52,458 | 23,141–23,193 |
| Spring Boot | Native Image | 43,404 | 8,340 | +420.4% | 43,256–43,428 | 8,334–8,350 |
| Micronaut | JVM | 71,616 | 26,421 | +171.1% | 70,940–72,119 | 26,404–26,456 |
| Micronaut | Native Image | 61,354 | 8,880 | +590.9% | 61,131–62,147 | 8,873–8,881 |
| Ktor | JVM | 70,985 | 26,702 | +165.8% | 70,113–72,766 | 26,664–26,778 |
| Ktor | Native Image | 66,034 | 18,367 | +259.5% | 65,847–66,112 | 18,341–18,379 |

### Latency, CPU and server memory

Each cell lists **Bemo / Netty**. P99 latency and CPU cost are sample medians;
server peak RSS is the maximum measured lifetime high-water mark across
samples, including startup and warmup.

| Framework / runtime | Workload | P99 µs | Server CPU µs/req | Server peak RSS MiB |
| --- | --- | ---: | ---: | ---: |
| Spring Boot / JVM | plaintext | 442.0 / 451.0 | 13.2 / 13.0 | 395.8 / 384.9 |
| Spring Boot / Native | plaintext | 1,297.0 / 1,186.0 | 15.8 / 16.3 | 147.1 / 141.7 |
| Micronaut / JVM | plaintext | 273.0 / 258.0 | 8.2 / 7.6 | 333.6 / 334.1 |
| Micronaut / Native | plaintext | 719.0 / 704.0 | 10.0 / 10.1 | 129.9 / 124.3 |
| Ktor / JVM | plaintext | 327.0 / 356.0 | 9.0 / 8.3 | 316.7 / 327.7 |
| Ktor / Native | plaintext | 653.0 / 636.0 | 10.3 / 10.0 | 112.2 / 107.9 |
| Spring Boot / JVM | payload | 1,159.0 / 1,836.0 | 35.2 / 29.8 | 396.0 / 389.3 |
| Spring Boot / Native | payload | 1,951.0 / 1,852.0 | 38.6 / 38.2 | 153.8 / 141.0 |
| Micronaut / JVM | payload | 986.0 / 1,486.0 | 30.2 / 24.1 | 350.5 / 335.6 |
| Micronaut / Native | payload | 1,670.0 / 1,793.0 | 32.4 / 28.6 | 142.2 / 124.6 |
| Ktor / JVM | payload | 1,069.0 / 1,576.0 | 32.3 / 25.6 | 331.5 / 318.3 |
| Ktor / Native | payload | 1,427.0 / 1,949.0 | 33.7 / 39.6 | 120.0 / 116.1 |
| Spring Boot / JVM | compression | 1,361.0 / 5,114.0 | 34.7 / 83.1 | 408.9 / 387.7 |
| Spring Boot / Native | compression | 1,980.0 / 14,469.0 | 40.3 / 234.1 | 151.1 / 142.5 |
| Micronaut / JVM | compression | 950.0 / 4,552.0 | 26.3 / 74.4 | 339.0 / 335.0 |
| Micronaut / Native | compression | 1,356.0 / 13,721.0 | 28.9 / 221.9 | 134.8 / 125.2 |
| Ktor / JVM | compression | 948.0 / 2,798.0 | 26.3 / 74.3 | 323.3 / 327.0 |
| Ktor / Native | compression | 1,194.0 / 8,111.0 | 28.1 / 114.2 | 116.9 / 109.3 |
| Spring Boot / JVM | tls | 1,889.0 / 3,005.0 | 50.0 / 48.8 | 425.8 / 402.1 |
| Spring Boot / Native | tls | 2,982.0 / 3,651.0 | 58.5 / 59.3 | 170.8 / 156.5 |
| Micronaut / JVM | tls | 1,464.0 / 2,357.0 | 44.0 / 38.3 | 365.9 / 335.4 |
| Micronaut / Native | tls | 3,047.0 / 3,065.0 | 50.7 / 49.1 | 156.1 / 140.1 |
| Ktor / JVM | tls | 1,474.0 / 2,341.0 | 41.6 / 38.4 | 350.8 / 324.3 |
| Ktor / Native | tls | 2,315.0 / 2,739.0 | 45.4 / 44.4 | 137.9 / 123.1 |
| Spring Boot / JVM | tls-compression | 1,562.0 / 5,314.0 | 38.5 / 87.0 | 426.6 / 398.1 |
| Spring Boot / Native | tls-compression | 2,215.0 / 14,744.0 | 45.4 / 239.1 | 158.3 / 158.0 |
| Micronaut / JVM | tls-compression | 1,020.0 / 4,666.0 | 28.2 / 76.4 | 349.6 / 342.8 |
| Micronaut / Native | tls-compression | 1,530.0 / 13,903.0 | 32.4 / 225.1 | 142.6 / 139.5 |
| Ktor / JVM | tls-compression | 1,068.0 / 3,102.0 | 28.6 / 76.0 | 341.6 / 328.4 |
| Ktor / Native | tls-compression | 1,338.0 / 4,730.0 | 30.1 / 108.7 | 125.8 / 123.4 |

## Measurement controls

The basic matrix compares Bemo Native Image `-O3`, native HTTP, io_uring,
zlib-rs and Rustls/aws-lc-rs with OpenJDK Netty epoll, Netty HTTP and
tcnative/BoringSSL. These are complete server stacks with different runtimes.
Each sample uses the same external OpenJDK Netty NIO/JSSE client, four
persistent connections, 5,000 warmup rounds and 25,000 measured rounds per
connection. Every decoded response is checked. The twelve workloads cover
1 KiB, 64 KiB and 128 KiB JSON bodies, HTTP/HTTPS and identity/gzip.
Server and client heaps are 256 MiB; each server has one I/O thread.

The framework matrix holds the application and runtime fixed within each
comparison: Spring Boot, Micronaut and Ktor, each on JVM and Native Image.
Framework HTTP codecs remain in use. Bemo supplies io_uring, zlib-rs and
Rustls; stock mode uses epoll, a reusable JDK Deflater and tcnative/BoringSSL.
Each application artifact supports both modes. Native Images use `-O3`, the
portable default `x86-64-v3` target and no trained PGO profile.
Each sample uses four wrk threads, 64 persistent connections, 20 seconds
of warmup and 20 seconds measured. The five endpoints serve a 13-byte greeting
or a 128 KiB fixed ASCII body, with identity/gzip and HTTP/HTTPS variants.
The validating wrk callback checks decoded contents, gzip CRC and provider
headers on every response. Validation is included in throughput.

Gzip level is **1 on both sides**. This holds the level constant, not the
compressed size or codec. TLS clients use **TLS 1.3 / TLS_AES_128_GCM_SHA256**.
The JSSE client checks negotiated sessions. Framework readiness checks the
protocol/cipher; the benchmark-only OpenSSL shim constrains wrk's TLS contexts.
TLS samples reuse connections and principally measure record processing,
not fresh-handshake throughput.

Unclemax is a shared Linux x86-64 Threadripper PRO 9965WX host, with 24 cores
and 48 threads. Servers use physical CPUs **12–17** and clients **18–21**,
in separate L3 groups without shared SMT siblings. The CPU governor is
`performance`. Existing background services remain running. Builds,
correctness tests, tracing and benchmark families run sequentially.
Transport launch order alternates across the three repetitions.

Sample ranges are observed minima and maxima, not confidence intervals.
Basic memory sums server/client lifetime RSS high-water marks, including
startup and warmup; it is not a simultaneous peak. Framework RSS is
server-only. The harnesses use different clients, concurrency and bodies;
their absolute request rates cannot be compared directly.

## Reproduce the cycle

Use a Linux host with the pinned Elide/JDK toolchain, Native Image, wrk,
OpenSSL development libraries and strace. Choose disjoint physical CPU groups
for the server and client, and keep unrelated builds off those groups during
timing. The following uses this cycle's affinity:

```sh
make build
make check
make test
make test-native-image
python3 tools/test_git_dependency.py
python3 tools/examples.py prepare
python3 tools/examples.py native --native-opt 3
python3 tools/examples.py smoke
python3 tools/examples.py smoke --native
make bench-prepare
python3 tools/bench_sizes.py
python3 tools/bench_frameworks.py --probe --workload tls-compression \
  --server-cpus 12-17 --client-cpus 18-21 \
  --output build/reports/framework-backend-probe
make bench
BEMO_BENCH_GZIP_LEVEL=1 BEMO_BENCH_SERVER_CPUS=12-17 \
  BEMO_BENCH_CLIENT_CPUS=18-21 python3 tools/bench.py run --backend 2
for workload in plaintext payload compression tls tls-compression; do
  python3 tools/bench_frameworks.py --workload "$workload" \
    --server-cpus 12-17 --client-cpus 18-21 \
    --output "build/reports/framework-$workload"
done
python3 tools/compression_probe.py --backends zlib-rs zlib zlib-ng
```

Framework output directories must be new. The basic harness rejects source or
artifact drift after preparation. Retain the full source snapshot and build
fingerprints when running a working tree. After collecting the complete raw
matrix and updating provenance, `make bench-graphs` reproduces the README SVGs.

## Evidence and qualification

The cycle began October 7 and finished October 8, 2026 (Pacific); recorded
process timestamps use UTC. Dataset filenames identify the cycle's start date.
The measured parent is `1d3ca121bd44d456cdbdadda5a2d46e5328d8517`, with the
[working-tree patch](data/native-source-20261007.patch) and
[365 source file hashes](data/native-source-20261007.json) retained. Applying
that patch to the parent reproduced every hash, with zero mismatches. The
working-tree changes were chart validation; application and timing code came
from the parent commit. The reporting renderer was subsequently extended for
this complete dataset.

- [Basic raw samples](data/native-basic-20261007.json) and [chart provenance](data/provenance.json).
- [All framework raw samples, summaries and environments](data/native-frameworks-20261007.json).
- [Prepared artifact fingerprints](data/native-build-20261007.json) and [environment, dependency hashes and post-cycle verification](data/native-environment-20261007.json).
- [Untimed native backend probes and syscall traces](data/native-backend-probe-20261007.json).
- [Validated compressed response sizes](data/native-sizes-20261007.json).
- [Standalone compression backends](data/native-compression-20261007.json): 162 compression samples across zlib-rs, zlib and zlib-ng, plus 36 checksum samples.
- [Rust Criterion estimates](data/native-criterion-20261007.json): 72 Bemo-only cases using default sampling and unpinned affinity, without a Netty comparator.
- [Executed cycle commands](data/native-cycle-20261007.sh) and [raw log archive checksum](data/native-archive-20261007.json). The archive is retained under `build/native-cycle` locally and on the benchmark host.

The post-cycle check matched every prepared source, class, shared-library and
native HTTP executable hash. All five framework workloads used the same
application artifacts. Every stock sample identified an actual
`EpollServerSocketChannel`; TLS samples identified tcnative/BoringSSL. The
untimed traces showed successful io_uring setup and active `io_uring_enter`
calls in all six Bemo framework/runtime modes, and active epoll waits in all
six stock modes. Tracing ended before timed sampling.

Linux qualification passed `make build`, `make check`, `make test`,
`make test-native-image`, the consuming Cargo Git-dependency test, and all twelve
Elide framework smoke modes. Smoke tests cover payload integrity, gzip, TLS
1.2/1.3, combined TLS/gzip, concurrency, keep-alive and resource reclamation.
The baseline control also rejected startup when either the native transport or
tcnative library was removed. Rendering and the 74 Python tests passed against
the refreshed evidence.

The pinned stack is Netty **4.2.18.Final**, tcnative **2.0.84.Final**,
Elide **1.5.4+20260921**, OpenJDK **25.0.2**, and GraalVM **25.3.4.1+1.1**.
Stock Native Images contain the same optional Bemo code as the Bemo mode;
both modes run from one executable per framework.

## Compression size controls

Body sizes exclude headers, chunk framing and TLS framing. Each basic size
was verified through three persistent responses and independently decoded.

| Original JSON body | Bemo level 1 | Netty level 1 | Netty level 6 control |
| --- | ---: | ---: | ---: |
| 1 KiB | 83 B | 90 B | 88 B |
| 64 KiB | 859 B | 558 B | 288 B |
| 128 KiB | 1,653 B | 1,002 B | 479 B |

The framework ASCII body is 1,580 B with Bemo versus 909 B with stock gzip,
including when carried over TLS. All timed gzip samples use level 1.
