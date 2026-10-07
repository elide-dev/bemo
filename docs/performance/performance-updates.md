# Performance follow-ups: full transport results

TLS record coalescing, direct header encoding, and native gzip are measured at
`a87e7abcec4ee5b317a9212b7839a1d04fc0702d`, compared with the merged five-PR stack
commit `d46a3658547654490cf5fcb1c063242728df7f99`. The tables below compare the two versions.
The current README also includes the subsequent level/TLS changes and 128 KiB
workloads; see [scaling results](scaling-results.md).

- Before: [before follow-ups](https://github.com/elide-dev/bemo/actions/runs/37581322288), [provenance](data/pre-follow-up-provenance.json), [raw samples](data/pre-follow-up-linux-x86_64.json).
- After: [after follow-ups](https://github.com/elide-dev/bemo/actions/runs/37581322288), [provenance](data/first-follow-up-provenance.json), [raw samples](data/first-follow-up-linux-x86_64.json).

Both versions use Linux x86-64, Native Image `-O3`, native HTTP and Rustls/AWS-LC
with io_uring against stock OpenJDK Netty epoll/JDK TLS. Every workload has
three samples, four persistent clients, 5,000 warmup rounds, and 25,000 measured
rounds per client (100,000 completed requests). Runtime versions, socket settings,
and transport labels match. Bemo gzip changes from java.util.zip to the pinned
zlib-rs provider; Netty retains its compressor. Workload-source fingerprints differ
because they include the optimized server. The load-generator/Netty source and
sampling logic are unchanged. Measurement metadata now records the provider
reported by the native server.

Both versions were rebuilt before measurement on one GitHub-hosted Linux
runner. Commit order alternates by workload/sample; Bemo/Netty order alternates
by sample. [Shared runner evidence](data/first-follow-up-environment.json) records CPU
and allowed affinity. Shared-host scheduling still varies. Ranges span three
samples and are not confidence intervals. These results measure the combined follow-ups.

## Throughput

| Workload | Bemo before RPS (min–max) | Bemo after RPS (min–max) | Bemo change | Netty change | Bemo/Netty before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| HTTP · identity · 1 KiB | 27,655 (27,415–27,877) | 26,931 (26,204–27,673) | -2.6% | +0.3% | 1.04× → 1.01× |
| HTTP · identity · 64 KiB | 18,683 (18,624–18,705) | 18,650 (18,428–18,853) | -0.2% | -0.4% | 1.04× → 1.05× |
| HTTP · gzip · 1 KiB | 16,802 (16,179–18,522) | 18,572 (17,405–19,030) | +10.5% | -12.3% | 1.12× → 1.41× |
| HTTP · gzip · 64 KiB | 2,710 (2,695–2,713) | 9,529 (9,410–9,532) | +251.6% | +0.0% | 1.13× → 3.96× |
| TLS · identity · 1 KiB | 19,279 (18,905–19,715) | 24,577 (24,049–24,903) | +27.5% | +1.0% | 1.00× → 1.27× |
| TLS · identity · 64 KiB | 8,601 (8,558–8,638) | 8,294 (8,233–8,364) | -3.6% | +0.1% | 0.94× → 0.91× |
| TLS · gzip · 1 KiB | 13,555 (12,962–14,417) | 16,729 (15,974–16,824) | +23.4% | +2.4% | 1.12× → 1.35× |
| TLS · gzip · 64 KiB | 2,455 (2,447–2,471) | 8,735 (8,714–8,922) | +255.9% | -1.7% | 1.05× → 3.79× |

## Latency, CPU, and memory

Latency and server CPU are sample medians. Memory is the maximum sample sum
of server/client lifetime RSS high-water marks, including startup and warmup;
it is not server-only RSS, heap size, or a simultaneous peak.

| Workload | p99 before → after (µs) | Server CPU before → after (µs/request) | Total RSS before → after (MiB) |
| --- | ---: | ---: | ---: |
| HTTP · identity · 1 KiB | 155.7 → 162.1 | 18.1 → 19.2 | 294.1 → 286.9 |
| HTTP · identity · 64 KiB | 251.1 → 250.0 | 27.8 → 27.8 | 291.1 → 289.1 |
| HTTP · gzip · 1 KiB | 265.0 → 261.4 | 35.4 → 31.0 | 334.4 → 307.2 |
| HTTP · gzip · 64 KiB | 1,545.0 → 507.5 | 307.7 → 50.1 | 331.3 → 305.3 |
| TLS · identity · 1 KiB | 271.3 → 220.6 | 30.3 → 20.4 | 323.7 → 318.4 |
| TLS · identity · 64 KiB | 618.7 → 643.4 | 77.0 → 79.6 | 322.3 → 320.7 |
| TLS · gzip · 1 KiB | 369.3 → 306.9 | 43.8 → 33.6 | 352.5 → 324.0 |
| TLS · gzip · 64 KiB | 1,729.9 → 559.8 | 323.7 → 53.7 | 356.2 → 334.1 |

## Reading the result

Large gzip is the clearest gain: HTTP/TLS throughput rises 251.6%/255.9%,
server CPU falls from 307.7/323.7 to 50.1/53.7 µs per request, and p99 falls
about 67–68%. Small TLS identity improves 27.5%, with CPU down 32.7%; small
TLS gzip improves 23.4%. Small HTTP gzip improves 10.5%, but its ranges overlap
and Netty drops 12.3%, making that case a weaker comparison.

Plain identity has no demonstrated gain: 1 KiB drops 2.6% with overlapping
ranges, and 64 KiB changes -0.2%. Large TLS identity drops 3.6%, with disjoint
three-sample ranges, CPU up 3.4%, and p99 up 4.0%. This is a measured cost to
investigate rather than a universal speedup. Coalescing a header with an exact
multiple of 16 KiB in the body preserves the record count while copying a prefix;
a focused comparison should test skipping that staging. The combined run does
not isolate its contribution. Netty varies -1.7% to +2.4% in the other cases.

Speed is the primary provider criterion. The compressed-size increase is
reported in the backend qualification as context. Combined lifetime RSS falls
in all eight cases, most noticeably for gzip; it does not isolate compressor
state or server memory.

Native HTTP gzip now resets reusable zlib-rs state and returns an independent
frozen output for every response, through both bindings. It directly pins
[the upstream teardown fix](https://github.com/trifectatechfoundation/zlib-rs/pull/555)
at `cedb23f2a9329d0bc81d0c8068d74d2b16a24dd6` (0.6.7).
The standalone Linux probe uses the same revision; see
[compression qualification](compression.md). Changing input, independent JDK
decoding, output budgets, thread affinity, and teardown pass both runtime contracts
and Miri. This replaces the preceding benchmark provider, not Java Deflater itself.

TLS now fills a bounded plaintext record across adjacent retained parts. Original
leases remain live until ciphertext retirement. Record-count and peer-byte tests
cover 1 KiB, 16 KiB, and 64 KiB bodies with an empty part between header and body.
Headers now write directly to exact-size native storage, with checked sizing
and one cached Date lookup, avoiding the temporary vector and copy.

Same-host [lease qualification](lease-bookkeeping.md) found changes of +0.2%
for retain/drop, -0.5% for slice/drop, and +0.2% for exclusive recovery. These
6–10 ns uncontended operations do not justify another ownership representation
change.

The HTTP load generator does not force one-byte or seven-byte header fragments,
and it does not isolate registry or shared-view operations. Those individual
optimization costs require their dedicated microbenchmarks.

The preceding README snapshot measured the five-PR stack at `a0537b4`.
Its code is the same baseline as `d46a365`;
[the earlier eight-case report](optimization-results.md) and its raw data remain archived.

## Validation

The same CI run passed Rust on Linux/macOS/Windows, JVM FFM and C binding
contracts on Linux/macOS with JDK 22/25, Native Image contracts, ASan, TSan,
Miri, and fuzzing. The ownership and compression probes passed independently.
Local `make build`, `make check`, `make test`, `make test-native-image`,
formatting, external Cargo consumer checks, and the Python tooling tests passed.

Source SHA-256 before: `3d1fa2b4123abff8c2038f2fd998cd4a1c665eabae11dbdb8affc98e21358d26`.
Source SHA-256 after: `b5f4520580b88363028d6110336229ec466081375a41098c8d76e7814c416de6`.
