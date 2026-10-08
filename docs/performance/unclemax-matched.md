# Matched gzip-level benchmarks on Unclemax

Baseline note: these archived framework measurements used Netty NIO and JDK
TLS. The current harness requires native epoll/kqueue and tcnative/BoringSSL.
These numbers must not be presented as results against that native baseline;
see the [native epoll/tcnative cycle](native-baseline.md) for the corrected baseline.

Measured with gzip level **1 on both sides**, using an isolated working-tree
snapshot based on `a8bf91d49ea7719ab8a4460aa22f8d97bf93ae51`.
The parent commit alone does not identify the measured source. Per-file hashes,
prepared artifact hashes, environment details, and every timed sample are retained:

- [Source snapshot](data/unclemax-level1-source.json), [basic build fingerprints](data/unclemax-level1-build.json), and [environment](data/unclemax-level1-environment.json).
- [Basic raw samples](data/linux-x86_64.json) and [archived chart provenance](data/provenance-jdk.json).
- [Framework raw samples, summaries, and build environments](data/framework-unclemax-level1.json).
- [Validated HTTP compression sizes](data/compression-sizes-unclemax-level1.json), [standalone compression backends](data/compression-backends-unclemax-level1.json), and [TLS client policy controls](data/tls-policy-unclemax-level1.json).
- [Bemo-only Rust microbenchmarks](data/criterion-unclemax-level1.json): 72 cases with default Criterion settings, without a Netty comparator or pinned affinity.

## Response sizes

These are compressed HTTP body bytes, excluding HTTP headers, chunk framing,
and TLS framing. Each cell was checked across three persistent-connection
responses and independently decoded to the exact original payload.

| Original JSON body | Bemo zlib-rs level 1 | Netty level 1 | Netty level 6 (size control) |
| --- | ---: | ---: | ---: |
| 1 KiB | 83 | 90 | 88 |
| 64 KiB | 859 | 558 | 288 |
| 128 KiB | 1,653 | 1,002 | 479 |

Level 1 native gzip is smaller than Netty level 6 only for the 1 KiB body.
At 128 KiB it is 3.45 times the level-6 body, and 1.65 times the level-1 body.
All current throughput comparisons use level 1 throughout. Matching levels
does not match the codecs’ compression ratios; these results retain that cost.

## Basic HTTP/TLS matrix

Bemo Native Image `-O3` uses native HTTP, io_uring and Rustls/aws-lc-rs;
the comparator uses stock OpenJDK Netty epoll, Netty HTTP and JDK TLS.
This is a full-stack comparison with different server runtimes. The common
external OpenJDK NIO client has four persistent connections and validates every
decoded response. Both processes have 256 MiB heaps. Each fresh-server sample
warms 5,000 rounds per client and measures 25,000 rounds per client.
Three samples per transport complete 7,200,000 measured requests across 12 cases.

| Workload | Bemo req/s | Netty epoll req/s | Bemo change | Bemo range | Netty range |
| --- | ---: | ---: | ---: | ---: | ---: |
| plain-identity-1024 | 98,469 | 88,580 | +11.2% | 96,386–100,326 | 86,463–89,856 |
| plain-identity-65536 | 53,193 | 50,904 | +4.5% | 52,845–53,834 | 50,383–51,070 |
| plain-identity-131072 | 36,352 | 33,157 | +9.6% | 36,236–36,523 | 33,014–33,762 |
| plain-gzip-1024 | 85,612 | 61,963 | +38.2% | 82,258–86,094 | 61,918–61,971 |
| plain-gzip-65536 | 35,540 | 21,429 | +65.9% | 35,473–35,924 | 21,259–21,450 |
| plain-gzip-131072 | 22,228 | 11,029 | +101.5% | 22,128–22,272 | 10,982–11,071 |
| tls-identity-1024 | 85,325 | 73,680 | +15.8% | 84,578–85,514 | 72,893–73,966 |
| tls-identity-65536 | 22,571 | 26,204 | -13.9% | 22,438–22,799 | 26,157–26,294 |
| tls-identity-131072 | 13,604 | 15,042 | -9.6% | 13,595–13,653 | 15,003–15,254 |
| tls-gzip-1024 | 74,317 | 54,799 | +35.6% | 73,549–74,726 | 54,530–55,178 |
| tls-gzip-65536 | 33,046 | 20,208 | +63.5% | 33,010–33,396 | 20,071–20,222 |
| tls-gzip-131072 | 20,910 | 10,738 | +94.7% | 20,857–20,929 | 10,674–10,798 |

The archived [throughput](graphs/throughput-29652ad54cad.svg),
[payload/latency](graphs/payload-curves-29652ad54cad.svg),
[CPU/memory](graphs/efficiency-29652ad54cad.svg) and
[framework](graphs/framework-throughput-afcab56b5053.svg) charts retain these results.
They show p99 latency, server CPU per completed request,
and the maximum sample sum of server/client lifetime RSS high-water marks.
That memory figure includes startup and warmup and is not a simultaneous peak.

## Framework matrix

All 120 measured samples completed 169,568,257 responses, with zero client errors, invalid responses, or backend fallbacks.

Spring Boot 4.1.1 and Micronaut 4.10.30 keep their framework HTTP codecs.
Both use Netty 4.2.18.Final. Each comparison below holds the runtime fixed.
Bemo uses io_uring, native zlib-rs gzip and Rustls/aws-lc-rs TLS. Stock mode
uses Netty NIO, a reusable JDK Deflater gzip helper and JDK TLS; this stock
compression helper is distinct from the basic harness’s HttpContentCompressor.
All gzip providers use level 1 and compress a fresh response on each request.
All timed TLS clients use TLS 1.3 / `TLS_AES_128_GCM_SHA256`. The basic
JDK client verifies every negotiated session. Framework readiness verifies
the protocol/cipher, and an OpenSSL context policy shim enforces it in wrk.
The shim is preloaded only into wrk, never into application servers.
An AES-256-only server is rejected by the actual wrk client; an AES-128-only
server transfers TLS application data. OpenSSL development headers are needed
to rebuild the Linux client policy shim through `make bench-prepare`.

Elide builds both projects. Each framework/runtime pair uses the same prepared
artifact for both transport modes, toggled with `bemo.enabled`; the stock Native
Image therefore still contains the optional Bemo code. Native Images use `-O3`;
JVM apps use OpenJDK 25.0.2. The Native Image target is the portable default
`x86-64-v3`, with ML-inferred PGO and no trained profile or `-march=native`.
The native TLS results describe this target and provider pair; they do not
establish a speedup over every CPU-specific Native Image build.
Each server has two I/O workers and a 256 MiB heap. Bemo shares its worker
group with the acceptor; stock mode retains framework acceptor defaults.
The common wrk client uses
four threads and 64 persistent connections, with 20 seconds of warmup followed
by 20 seconds of measurement. Every fresh-server case has three repetitions.
The validating client independently inflates every gzip body, checking CRC,
exact decoded contents and provider headers. Identity bodies are also checked.
This validation is included in throughput; TLS wrk uses its OpenSSL stack.
HTTPS readiness separately checks the certificate. The 13-byte plaintext body
is `Hello, World!`; all large bodies repeat the same ASCII pattern to 128 KiB.
Basic JSON and framework ASCII workloads have different compression ratios.
Their different clients/concurrency also prevent direct cross-harness RPS comparisons.
The captured wire-body sizes are available with every framework sample.
For this ASCII body, gzip outputs are 1,580 bytes with Bemo and 909 bytes
with the stock JDK helper; TLS framing is excluded. These differ from the
basic JSON body above. Both providers use level 1.
Startup includes two verified replies and 50 ms polling; RSS is server-only.
JVM G1 and Native Image Serial GC defaults are retained.
TLS benchmarks reuse persistent connections; they primarily exercise records.
They do not represent fresh-handshake throughput.

### plaintext

| Framework | Runtime | Bemo req/s | Stock req/s | Bemo change | Bemo range | Stock range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 154,058 | 153,193 | +0.6% | 152,146–158,241 | 150,190–153,559 |
| Spring Boot | Native Image | 125,784 | 115,946 | +8.5% | 125,501–126,171 | 115,844–117,214 |
| Micronaut | JVM | 248,119 | 257,823 | -3.8% | 245,860–253,265 | 256,539–258,888 |
| Micronaut | Native Image | 199,439 | 189,023 | +5.5% | 199,383–200,825 | 186,362–189,921 |

### payload

| Framework | Runtime | Bemo req/s | Stock req/s | Bemo change | Bemo range | Stock range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 56,736 | 66,408 | -14.6% | 56,325–57,189 | 66,059–68,923 |
| Spring Boot | Native Image | 51,192 | 51,601 | -0.8% | 51,183–51,326 | 50,920–51,930 |
| Micronaut | JVM | 67,264 | 86,092 | -21.9% | 67,256–67,695 | 82,070–86,256 |
| Micronaut | Native Image | 62,155 | 67,342 | -7.7% | 62,055–62,467 | 66,049–69,261 |

### compression

| Framework | Runtime | Bemo req/s | Stock req/s | Bemo change | Bemo range | Stock range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 58,292 | 23,917 | +143.7% | 58,003–58,457 | 23,757–24,112 |
| Spring Boot | Native Image | 49,675 | 8,525 | +482.7% | 49,574–49,752 | 8,522–8,528 |
| Micronaut | JVM | 77,369 | 27,017 | +186.4% | 77,044–78,164 | 27,002–27,036 |
| Micronaut | Native Image | 69,225 | 8,996 | +669.5% | 68,582–69,677 | 8,990–9,003 |

### tls

| Framework | Runtime | Bemo req/s | Stock req/s | Bemo change | Bemo range | Stock range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 40,425 | 35,717 | +13.2% | 40,128–40,914 | 35,586–35,726 |
| Spring Boot | Native Image | 33,795 | 1,512 | +2135.5% | 33,751–34,256 | 1,511–1,514 |
| Micronaut | JVM | 45,453 | 44,251 | +2.7% | 44,781–46,085 | 43,912–44,413 |
| Micronaut | Native Image | 38,822 | 1,521 | +2452.2% | 38,728–39,671 | 1,521–1,522 |

### tls-compression

| Framework | Runtime | Bemo req/s | Stock req/s | Bemo change | Bemo range | Stock range |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Spring Boot | JVM | 52,632 | 22,830 | +130.5% | 52,042–52,796 | 22,794–22,859 |
| Spring Boot | Native Image | 44,075 | 7,909 | +457.3% | 43,854–44,537 | 7,906–7,918 |
| Micronaut | JVM | 71,659 | 26,152 | +174.0% | 69,871–72,003 | 26,091–26,237 |
| Micronaut | Native Image | 61,313 | 8,356 | +633.7% | 61,048–61,899 | 8,356–8,357 |

## Environment and limits

Unclemax is a Linux x86-64 Threadripper PRO 9965WX host (24 cores/48 threads).
Servers are pinned to physical CPUs 12–17; clients to 18–21, separate L3 groups
without shared SMT siblings. The governor and energy preference are performance.
The host remains shared: existing background workloads were preserved and
captured. No builds, correctness suites, or other benchmark families overlap
these HTTP measurements. Both transports receive the same CPU sets.
Transport launch order alternates across samples; framework/runtime pairs rotate.
No backend fallback or client error is accepted. Three-sample ranges describe
observed variation, not confidence intervals or statistically established wins.
These closed-loop loopback measurements do not establish a universal speedup,
production network performance, or application/database throughput.

An initial level-matched pass used different default TLS ciphers: AES-256 for
Bemo and AES-128 for stock. Its TLS results are superseded by this rerun.
The non-TLS framework samples retain the [initial source snapshot](data/unclemax-level1-initial-source.json); TLS samples and basic workloads use
the normalized client harness snapshot. Application artifact hashes are unchanged.

The earlier [scaling results](scaling-results.md) used level 1 versus level 6
and a different host. They remain historical evidence and are superseded for
current README comparisons. A change versus those results cannot be attributed
solely to compression level. The [initial framework run](framework-initial.md)
tested only the plaintext endpoint.

## Validation

`make build`, `make check`, `make test`, `make test-native-image`, and the
external Cargo Git dependency contract passed on Unclemax before timing.
Both frameworks passed endpoint smoke checks in JVM and Native Image modes,
with Bemo and stock providers. The standalone compression sweep completed
162 gzip measurements and 36 checksum measurements with round-trip validation.
Final harness/chart tests passed all 59 cases; formatting and whitespace checks
passed, and chart regeneration reproduced identical SVG bytes.

## Reproduce

Builds and tests must finish before measuring. On a matching Linux toolchain:

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
make bench
BEMO_BENCH_GZIP_LEVEL=1 BEMO_BENCH_SERVER_CPUS=12-17 BEMO_BENCH_CLIENT_CPUS=18-21 \
  python3 tools/bench.py run --backend 2
for workload in plaintext payload compression tls tls-compression; do
  python3 tools/bench_frameworks.py --workload "$workload" --gzip-level 1 \
    --server-cpus 12-17 --client-cpus 18-21 --output "build/reports/framework-$workload"
done
python3 tools/compression_probe.py --backends zlib-rs zlib zlib-ng
```

Choose suitable disjoint CPU sets on other hosts. Existing output directories
must be archived before reruns. See [chart refresh instructions](README.md)
for transferring complete evidence and regenerating the SVGs.
