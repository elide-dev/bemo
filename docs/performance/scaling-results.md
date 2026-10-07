# Historical selective TLS and gzip results through 128 KiB

These historical gzip results use different compression levels. Current matched-level
measurements are reported in [the Unclemax rerun](unclemax-matched.md).

Before: `a4d3cc5bf0ce505335c8d7270e698a27e64318fc`; after: `5f99dca7f062c94ed8cc7ba68594929dc5bf87cd`.
This compares the preceding native zlib-rs level-6 stack with selective TLS
record coalescing and speed-oriented zlib-rs level 1. Netty stays at its default
level 6. Provider and levels are explicit; gzip comparisons include that tuning.

[Paired CI run](https://github.com/elide-dev/bemo/actions/runs/37588109048) rebuilt both versions before measuring on
one Linux x86-64 host. Version order alternates by case/sample; transport order
alternates by sample. Each case/transport/version has three samples, four
persistent clients, 5,000 warmup rounds and 25,000 measured rounds per client.
The matrix contains 144 samples and 14.4 million completed measured requests.
Ranges are three observed samples, not confidence intervals.

- [Before provenance](data/pre-scaling-provenance.json) and [raw samples](data/pre-scaling-linux-x86_64.json).
- [After provenance](data/scaling-provenance.json) and [raw samples](data/scaling-linux-x86_64.json).
- [Shared CPU, affinity, commits, and requested workloads](data/paired-environment.json).

Native Image `-O3`, native HTTP, Rustls/AWS-LC and io_uring are compared with
stock OpenJDK Netty epoll/JDK TLS using the same external NIO client. Runtime
versions, workload sizes, socket settings and sampling logic match. Workload
fingerprints change with the optimized server and added level metadata.
Older CLI registries are extended in memory for the 128 KiB case; both older
server and client already accept variable sizes and are rebuilt unchanged.

## Throughput

| Workload | Bemo before RPS (range) | Bemo after RPS (range) | Bemo change | Netty change | Current Bemo / Netty |
| --- | ---: | ---: | ---: | ---: | ---: |
| HTTP · identity · 1 KiB | 48,283 (45,149–48,373) | 48,923 (45,277–49,254) | +1.3% | -2.0% | 1.04× |
| HTTP · identity · 64 KiB | 24,926 (24,698–25,129) | 24,687 (24,301–24,875) | -1.0% | +0.0% | 1.02× |
| HTTP · identity · 128 KiB | 17,621 (17,530–17,650) | 17,294 (16,963–17,384) | -1.9% | -0.5% | 1.01× |
| HTTP · gzip · 1 KiB | 29,540 (27,809–30,332) | 32,992 (32,719–35,195) | +11.7% | +3.1% | 1.27× |
| HTTP · gzip · 64 KiB | 12,123 (12,120–12,211) | 13,101 (13,029–13,614) | +8.1% | -4.6% | 3.55× |
| HTTP · gzip · 128 KiB | 7,098 (6,932–7,306) | 7,704 (7,694–7,801) | +8.5% | +1.6% | 4.00× |
| TLS · identity · 1 KiB | 37,967 (37,847–38,821) | 37,795 (36,200–37,971) | -0.5% | +1.1% | 1.03× |
| TLS · identity · 64 KiB | 12,803 (12,789–12,810) | 12,603 (12,579–12,829) | -1.6% | +0.5% | 0.95× |
| TLS · identity · 128 KiB | 8,086 (8,009–8,157) | 8,039 (7,970–8,075) | -0.6% | -1.1% | 1.03× |
| TLS · gzip · 1 KiB | 25,154 (24,916–25,164) | 29,217 (28,235–29,223) | +16.2% | -2.2% | 1.31× |
| TLS · gzip · 64 KiB | 11,338 (10,958–11,345) | 12,440 (12,353–12,513) | +9.7% | -2.0% | 3.39× |
| TLS · gzip · 128 KiB | 6,915 (6,800–7,056) | 7,711 (7,586–7,718) | +11.5% | +2.0% | 3.92× |

## Does performance hold as payload grows?

Logical payload MiB/sec multiplies median completed RPS by the original response
size. For gzip it measures decoded application bytes, not compressed wire bytes
or network bandwidth. Doubling payload may lower RPS while maintaining this
rate. This is a fixed-pattern, closed-loop workload, not a saturation sweep.

| Stack / workload | 64 KiB RPS → 128 KiB RPS | 64 KiB → 128 KiB logical MiB/sec | Logical throughput change |
| --- | ---: | ---: | ---: |
| Bemo · HTTP · identity | 24,687 → 17,294 | 1,542.9 → 2,161.8 | +40.1% |
| Bemo · HTTP · gzip | 13,101 → 7,704 | 818.8 → 963.0 | +17.6% |
| Bemo · TLS · identity | 12,603 → 8,039 | 787.7 → 1,004.9 | +27.6% |
| Bemo · TLS · gzip | 12,440 → 7,711 | 777.5 → 963.9 | +24.0% |
| Netty · HTTP · identity | 24,148 → 17,072 | 1,509.2 → 2,133.9 | +41.4% |
| Netty · HTTP · gzip | 3,686 → 1,925 | 230.3 → 240.6 | +4.5% |
| Netty · TLS · identity | 13,310 → 7,772 | 831.9 → 971.5 | +16.8% |
| Netty · TLS · gzip | 3,673 → 1,965 | 229.6 → 245.6 | +7.0% |

## CPU, latency, and memory

Server CPU and p99 are medians. RSS is the maximum sample sum of server/client
lifetime high-water marks, including startup/warmup, rather than a simultaneous
peak or server-only memory.

| Workload | Server CPU before → after (µs/request) | p99 before → after (µs) | Total RSS before → after (MiB) |
| --- | ---: | ---: | ---: |
| HTTP · identity · 1 KiB | 8.8 → 8.7 | 94.3 → 88.8 | 289.8 → 291.8 |
| HTTP · identity · 64 KiB | 17.3 → 17.6 | 173.5 → 178.6 | 300.3 → 298.2 |
| HTTP · identity · 128 KiB | 26.0 → 26.4 | 251.2 → 254.5 | 292.9 → 308.6 |
| HTTP · gzip · 1 KiB | 18.1 → 14.0 | 169.5 → 137.9 | 302.3 → 300.2 |
| HTTP · gzip · 64 KiB | 33.7 → 27.5 | 383.9 → 346.1 | 310.2 → 316.0 |
| HTTP · gzip · 128 KiB | 57.1 → 47.0 | 676.0 → 610.0 | 312.0 → 308.6 |
| TLS · identity · 1 KiB | 11.6 → 11.6 | 120.5 → 127.6 | 321.7 → 324.2 |
| TLS · identity · 64 KiB | 43.7 → 43.6 | 364.0 → 369.7 | 324.8 → 331.0 |
| TLS · identity · 128 KiB | 75.3 → 75.1 | 589.1 → 599.0 | 317.9 → 322.7 |
| TLS · gzip · 1 KiB | 21.6 → 16.6 | 200.7 → 164.2 | 324.2 → 324.9 |
| TLS · gzip · 64 KiB | 36.5 → 29.4 | 422.1 → 384.5 | 334.5 → 332.9 |
| TLS · gzip · 128 KiB | 59.5 → 46.6 | 714.7 → 642.1 | 334.7 → 337.4 |

## Interpretation

Gzip throughput improves in every measured size: HTTP gains 11.7% / 8.1% /
8.5% and TLS gains 16.2% / 9.7% / 11.5% at 1 / 64 / 128 KiB. Each gzip
after-range is above its before-range. Server CPU per gzip request falls by
about 18–23%, and median p99 improves in all six gzip cases. At 128 KiB, Bemo
reaches 7,704 HTTP and 7,711 TLS gzip RPS, approximately 4× the Netty comparator.
That comparison includes speed-oriented level 1 versus Netty’s default level 6.

Identity medians change between -1.9% and +1.3%; this pass does not establish
the expected TLS throughput recovery. TLS identity at 64 KiB declines 1.6%,
with overlapping sample ranges and nearly unchanged server CPU (43.7 → 43.6
µs/request). The selective-copy change is retained with its ownership and
peer-byte contracts, but should not be credited with a demonstrated RPS gain.
HTTP identity at 128 KiB also declines 1.9%, with non-overlapping ranges.

The 128 KiB measurements support sustained payload throughput for these shapes.
Bemo’s logical MiB/sec rises by 40.1% for HTTP identity, 27.6% for TLS identity,
17.6% for HTTP gzip and 24.0% for TLS gzip as payload doubles from 64 KiB.
Requests/sec decreases because each response does more work. These are measured
fixed-pattern results, rather than a prediction for arbitrary payload mixes.

Selective TLS stages a record only when packing remaining retained parts can
reduce their total record count. A header followed by an exact multiple of
16 KiB bypasses the prefix copy. Small responses still coalesce, and original
leases stay live through physical ciphertext retirement. Peer-byte, first-record
size and ownership tests cover 1 KiB, boundary sizes, 64 KiB and 128 KiB.

The [zlib-rs level sweep](compression.md) selected level 1 for speed. It changes
the benchmark/application recommendation; the native API continues to accept
an explicit level 0–9. Every response is freshly compressed from frozen input.
Independent JDK decoding and both runtime contracts cover changing 128 KiB
members at levels 1/3/6, wrong-thread rejection, reset, output budgets and teardown.

## SIMD investigation

The pinned zlib-rs match and sliding-hash paths already choose AVX2/BMI or NEON
at runtime; the HTTP parser has AVX2/SSE paths with a fallback. CRC32 remains
a useful bulk target as compression becomes faster. The standalone level sweep
also measures the current crc32fast path against zlib-rs CRC, with equal-result
checks. Those existing paths are close; wider CRC needs its own measured gain.
The pinned wider CRC/match variants are compile-time gated. Portable artifacts
must use runtime guards for every required feature and retain older-CPU paths.
No global native CPU target or unconditional advanced instruction was introduced.

Additional opportunity details and CodSpeed profile links remain in the local,
uncommitted optimization notes. The CodSpeed profiles consulted for this investigation
predate these changes, so they do not establish the current CPU shares.

## Validation

The paired run passed cross-platform Rust, both JVM bindings with JDK 22/25,
Native Image contracts, ASan, TSan, Miri, fuzzing and the lease qualification.
Local build/check/full tests/Native Image, external Cargo consumer, formatting,
and Python tooling checks passed. The preceding [follow-up report](performance-updates.md)
and [five-PR report](optimization-results.md) remain archived.

Before source SHA-256: `8ad0fb08b009881902743abaddc376f5c4474c789d6bf715f5b911826f1ad04d`.
After source SHA-256: `41b551ce8d99fcf0a44f60ee6e991b7aff99b8c9c1ddd018f170fc49b5301e26`.
