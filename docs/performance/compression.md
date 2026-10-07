# Native application gzip and backend comparison

The native HTTP benchmark compresses each gzip response through reusable
zlib-rs state in Bemo, with speed-oriented level 1 as its default.
`BEMO_BENCH_GZIP_LEVEL=6` selects the preceding native level for a control. The same Rust encoder is called from the stock JVM FFM
binding and Native Image C binding; Java's Deflater is no longer the default
native HTTP gzip provider. Netty retains its own application compressor.
`BEMO_BENCH_GZIP_PROVIDER=java.util.zip` selects the preceding reusable Java
implementation for an explicit diagnostic comparison. The server reports its
actual provider and the benchmark archives that label with every sample.

The additive `gzip_new(workload, level)`, `gzip_compress(workload, encoder, input)`,
and `gzip_release(encoder)` calls are owner-thread confined. Compression accepts
frozen input up to 16 MiB, resets at each response, and returns a new frozen
buffer handle charged to the workload. Output survives input/encoder release
and workload closure; callers release it with `buffer_release`. Closed or wrong
workloads, mutable input, stale handles, wrong threads, and exhausted output
budgets reject. Backend state and bounded scratch are internal overhead,
separate from the live output budget, as with TLS provider state.

`make test` and `make test-native-image` share a contract that roundtrips changing,
empty, repetitive, and incompressible members through the independent JDK gzip
decoder. It checks thread affinity, reset, budget exhaustion, immutable output,
and output lifetime after teardown. Rust ownership tests additionally run under
Miri. The prior Java provider's wire-equivalence tests remain for diagnostics.

The integration directly pins zlib-rs 0.6.7 at
`cedb23f2a9329d0bc81d0c8068d74d2b16a24dd6`, the fix in
[upstream PR #555](https://github.com/trifectatechfoundation/zlib-rs/pull/555).
The unpatched 0.6.8 safe wrapper triggers a Miri deallocation violation when
compressor state is passed by value to drop, matching
[issue #491](https://github.com/trifectatechfoundation/zlib-rs/issues/491).
The pinned fix holds provider state through raw pointers rather than persistent
mutable references. The declaration travels into consuming workspaces; no
root-only Cargo patch is required. Miri covers moving and releasing our encoder.
The encoder resets reusable Rust state and detects runtime CPU features.
zlib-rs remains the production provider. Historical backend comparisons below
are retained as measurement evidence.
Full-stack Linux throughput and CPU are the primary selection criteria;
compressed size is reported as context. Standalone compression speed alone
does not establish transport throughput.

## Alternative backends

Run `make bench-compression`, or `python3 tools/compression_probe.py --iterations 2000`
for a shorter level sweep. The default is zlib-rs at levels 1/3/6 and sizes
1/64/128 KiB. To reproduce the historical backend comparison, use explicit
`--backends zlib zlib-rs zlib-ng --levels 6`; those alternatives are standalone
probe features, and the application continues to use zlib-rs. Cargo builds each backend separately from a locked,
non-published workspace under `benchmarks/compression`. This standalone workspace compares providers separately from Bemo's integrated
zlib-rs encoder. Compilation precedes measurement; avoid
running builds or other benchmarks concurrently.

The probe compares raw DEFLATE with identical gzip framing and CRC32,
reusable compressor/output storage, 1,000 warmup iterations, and three samples.
It roundtrips each case before timing. The JSON pattern matches the HTTP workload;
deterministic random input exposes incompressible behavior. Raw samples and host
metadata are written to `build/reports/compression`.

An October 6, 2026 macOS ARM64 probe with 2,000 iterations per sample produced
these median costs. These are standalone backend measurements, excluding JVM,
FFM, Native Image, application scheduling, and network costs.

| Payload | zlib | zlib-rs | zlib-ng | Compressed bytes: zlib / rs / ng |
| --- | ---: | ---: | ---: | --- |
| 1 KiB JSON | 3,411 ns | 2,853 ns | 1,877 ns | 82 / 85 / 85 |
| 1 KiB random | 14,116 ns | 12,991 ns | 11,585 ns | 1,047 / 1,047 / 1,047 |
| 64 KiB JSON | 88,244 ns | 16,937 ns | 15,602 ns | 282 / 545 / 545 |
| 64 KiB random | 535,417 ns | 450,092 ns | 419,111 ns | 65,574 / 65,574 / 65,574 |

The original probe used flate2 1.1.10 and zlib-rs 0.6.8. The current probe uses
the same directly pinned zlib-rs teardown fix as Bemo; zlib and zlib-ng still
use flate2 1.1.10. Its lockfile pins the native zlib-ng dependency. The zlib backend uses the host's zlib. zlib-ng leads this local probe, with
zlib-rs close on large repetitive input. Both alternatives produce a larger result for that
input: CPU comparisons alone do not establish a full-stack win. The full-stack Linux benchmark must qualify the integrated provider.

[zlib-rs](https://github.com/trifectatechfoundation/zlib-rs) offers Rust and
zlib-compatible C APIs; [zlib-ng](https://github.com/zlib-ng/zlib-ng) offers a
native API and zlib compatibility mode. Switching this isolated Rust probe does
not switch Java's Deflater or GraalVM's linked zip implementation. The integrated zlib-rs path uses explicit native calls; it does not alter Java's
Deflater implementation.

The early local end-to-end before/after samples overlapped Native Image builds
and are unsuitable for performance claims. CodSpeed CI must establish the
application-level improvement of state reuse separately from backend selection.

## Linux qualification of the integrated revision

[The preceding paired CI run](https://github.com/elide-dev/bemo/actions/runs/37581322288) also measured all three providers
on its Linux x86-64 throughput host before the network samples. This uses the
same pinned zlib-rs revision as the integrated encoder, with 2,000 iterations
per sample and three samples per case. These standalone timings exclude
bindings, scheduling, and network costs.
[Raw samples](data/linux-compression.json) record the compiler, sizes, and every
observation; [provenance](data/compression-provenance.json) records the measured
commit and locked provider versions.

| Payload | zlib | zlib-rs | zlib-ng | Compressed bytes: zlib / rs / ng |
| --- | ---: | ---: | ---: | --- |
| 1 KiB JSON | 7,026 ns | 6,417 ns | 4,714 ns | 82 / 85 / 85 |
| 1 KiB random | 24,231 ns | 27,959 ns | 28,055 ns | 1,047 / 1,047 / 1,047 |
| 64 KiB JSON | 288,624 ns | 25,236 ns | 23,804 ns | 282 / 545 / 545 |
| 64 KiB random | 1,250,658 ns | 1,111,043 ns | 1,520,857 ns | 65,574 / 65,574 / 65,574 |

The application keeps zlib-rs; these historical alternatives are not integrated.
The full-stack [follow-up comparison](performance-updates.md) measures the
shipped zlib-rs path against the preceding reusable Java compressor. Selection
prioritizes speed over compressed size; the larger 64 KiB JSON output is
recorded as context.

## Level selection through 128 KiB

[The Linux level qualification](https://github.com/elide-dev/bemo/actions/runs/37587314983)
measured reusable zlib-rs at levels 1/3/6, with 1,000 warmup iterations and
three samples of 2,000 members each. Every case independently roundtrips before
timing. [Raw samples, compiler, CPU, and checksums](data/gzip-levels-linux-x86_64.json)
and [provenance](data/gzip-levels-provenance.json) retain the selection evidence.

| Payload | Level 1 | Level 3 | Level 6 | Compressed bytes: 1 / 3 / 6 |
| --- | ---: | ---: | ---: | --- |
| 1 KiB JSON | 2,612 ns | 6,367 ns | 6,305 ns | 83 / 84 / 85 |
| 1 KiB random | 8,918 ns | 27,163 ns | 28,107 ns | 1,098 / 1,047 / 1,047 |
| 64 KiB JSON | 16,564 ns | 24,098 ns | 25,183 ns | 859 / 545 / 545 |
| 64 KiB random | 516,103 ns | 1,053,030 ns | 1,129,567 ns | 69,126 / 65,574 / 65,574 |
| 128 KiB JSON | 33,011 ns | 44,245 ns | 46,039 ns | 1,653 / 990 / 990 |
| 128 KiB random | 1,194,949 ns | 2,281,494 ns | 2,459,294 ns | 138,261 / 131,130 / 131,130 |

Level 1 was fastest in all six shapes. The larger output on large bodies is
accepted in favor of speed. The [full transport scaling comparison](scaling-results.md)
qualifies the effect through 128 KiB against the same native provider at level 6.
The native API still requires an explicit level; this selects the HTTP benchmark
default and documents the application recommendation.

The checksum microbenchmarks compare equivalent IEEE CRC32 results separately
from gzip. At 64/128 KiB, crc32fast took approximately 5.4/10.9 µs and zlib-rs CRC
5.1/10.2 µs. That small difference does not establish a large provider-swap win.
It does show that checksum work is roughly a third of the fast level-1 encoder
cost, motivating a separately qualified runtime-dispatched wide checksum path.
