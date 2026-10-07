# Optimization stack: full transport results

The five optimization PRs are measured together at merged main commit
`a0537b44c9d17ac312e18adb08500cd1069913ed`, compared with the immediate pre-stack
commit `499a6f3dc4037fa8b7e264fe4928e7c3305b665a`. The archived stack data compares
Bemo with Netty within that run; the tables below compare the two versions.

- Before: [pre-stack CI run](https://github.com/elide-dev/bemo/actions/runs/37574875335), [provenance](data/pre-stack-provenance.json), [raw samples](data/pre-stack-linux-x86_64.json).
- After: [merged-main benchmark run](https://github.com/elide-dev/bemo/actions/runs/37574875335), [provenance](data/stack-provenance.json), [raw samples](data/stack-linux-x86_64.json).

Both versions use Linux x86-64, Native Image `-O3`, native HTTP and Rustls/AWS-LC
with io_uring against stock OpenJDK Netty epoll/JDK TLS. Every workload has
three samples, four persistent clients, 5,000 warmup rounds, and 25,000 measured
rounds per client (100,000 completed requests). Runtime versions, socket settings,
and transport/provider labels match. The workload-source fingerprints differ
because they include the optimized server. The load-generator/Netty source and
measurement function are unchanged; the preparation step adds the gzip helper.

Both versions were rebuilt before measurement on one GitHub-hosted Linux
runner. Commit order alternates by workload/sample; Bemo/Netty order alternates
by sample. [Shared runner evidence](data/stack-paired-environment.json) records CPU
and allowed affinity. This removes the cross-host mismatch of the first repeat,
while shared-host scheduling still varies. Ranges span three samples and are
not confidence intervals. These results measure the whole stack, not each PR.

## Throughput

| Workload | Bemo before RPS (min–max) | Bemo after RPS (min–max) | Bemo change | Netty change | Bemo/Netty before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| HTTP · identity · 1 KiB | 46,470 (45,752–47,818) | 47,294 (44,483–47,673) | +1.8% | -0.1% | 1.08× → 1.10× |
| HTTP · identity · 64 KiB | 25,113 (24,866–25,320) | 26,336 (26,096–26,913) | +4.9% | -2.2% | 1.03× → 1.11× |
| HTTP · gzip · 1 KiB | 11,031 (10,774–11,249) | 31,080 (30,626–31,090) | +181.7% | -5.9% | 0.43× → 1.29× |
| HTTP · gzip · 64 KiB | 3,214 (3,193–3,234) | 4,742 (4,731–4,776) | +47.5% | -1.5% | 0.77× → 1.16× |
| TLS · identity · 1 KiB | 37,897 (37,854–38,460) | 29,626 (29,501–31,292) | -21.8% | +0.2% | 1.12× → 0.87× |
| TLS · identity · 64 KiB | 12,301 (12,291–12,502) | 12,830 (12,743–12,852) | +4.3% | -0.4% | 0.89× → 0.94× |
| TLS · gzip · 1 KiB | 25,060 (24,832–26,021) | 22,020 (21,614–22,142) | -12.1% | +1.3% | 1.15× → 1.00× |
| TLS · gzip · 64 KiB | 3,932 (3,928–3,947) | 4,321 (4,286–4,323) | +9.9% | -1.1% | 0.99× → 1.10× |

## Latency, CPU, and memory

Latency and server CPU are sample medians. Memory is the maximum sample sum
of server/client lifetime RSS high-water marks, including startup and warmup;
it is not server-only RSS, heap size, or a simultaneous peak.

| Workload | p99 before → after (µs) | Server CPU before → after (µs/request) | Total RSS before → after (MiB) |
| --- | ---: | ---: | ---: |
| HTTP · identity · 1 KiB | 113.9 → 113.7 | 10.2 → 10.4 | 292.9 → 286.2 |
| HTTP · identity · 64 KiB | 186.4 → 180.3 | 24.8 → 22.6 | 294.4 → 301.5 |
| HTTP · gzip · 1 KiB | 444.4 → 170.9 | 72.9 → 18.2 | 371.2 → 329.4 |
| HTTP · gzip · 64 KiB | 1,332.8 → 934.4 | 268.2 → 171.2 | 375.1 → 335.4 |
| TLS · identity · 1 KiB | 150.9 → 185.4 | 13.0 → 19.7 | 327.6 → 324.1 |
| TLS · identity · 64 KiB | 406.3 → 386.0 | 52.1 → 49.4 | 323.1 → 322.2 |
| TLS · gzip · 1 KiB | 238.2 → 255.2 | 22.2 → 26.2 | 394.3 → 357.5 |
| TLS · gzip · 64 KiB | 1,100.7 → 1,021.7 | 212.8 → 179.0 | 392.4 → 355.1 |

## Reading the result

Plaintext gzip is the strongest gain: throughput rises 181.7% at 1 KiB and
47.5% at 64 KiB, while server CPU falls 75.0% and 36.2%, respectively.
TLS gzip at 64 KiB improves 9.9%, and identity at 64 KiB improves 4.3–4.9%.
The 1 KiB plaintext identity change is small (+1.8%) with overlapping ranges.

Small TLS responses regress: 1 KiB identity loses 21.8% throughput and gzip
loses 12.1%. Their server CPU rises from 13.0 to 19.7 and 22.2 to 26.2
µs/request, respectively; p99 also rises. Netty's matching cases change only
+0.2% and +1.3%. These regressions deserve follow-up rather than being hidden
behind the gzip wins. Combined gzip lifetime RSS falls roughly 37–42 MiB;
identity RSS changes are small and mixed.

Gzip still uses `java.util.zip` for per-response application compression.
The zlib-rs/zlib-ng probe did not change the shipped compression backend.
The retained response path sends its header and body as separate buffers.
The [TLS lane](../../crates/bemo/src/http/tls.rs) encrypts each buffer part
separately. A local probe on the measured commit confirmed one application
output for a contiguous 1,152-byte plaintext and two outputs for the same
bytes split at 128 bytes, with identical verified peer plaintext. This is a
candidate contributor to small-response TLS overhead; the probe does not
quantify its contribution to the full-stack results.

The HTTP load generator does not force one-byte or seven-byte header fragments,
and it does not isolate registry or shared-view operations. Those individual
optimization costs require their dedicated microbenchmarks.

The preceding README snapshot used commit `e69a09f` in
[this older run](https://github.com/elide-dev/bemo/actions/runs/37399667254).
The pre-stack baseline is used here to exclude changes predating the five PRs.

Source SHA-256 before: `d8245e78d62b46c4b710365e72279bf1606c8f97bbd254478f5ad6a587e23241`.
Source SHA-256 after: `59a0606e2497685b1efbd998ea7086a9b01fe06cb670ba76890d5ec34b0d499d`.
