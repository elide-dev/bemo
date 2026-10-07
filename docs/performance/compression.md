# Dynamic gzip reuse and backend comparison

The native HTTP benchmark compresses every gzip response. Its event-thread-owned
`ReusableGzip` keeps a Deflater, CRC32, and bounded-by-workload output array across
requests. Each call resets the stream and emits a complete independent gzip
member at the JDK's default compression level. Teardown releases the native
Deflater. Identity responses still use their existing path.

`make test` runs the gzip contract on the stock JVM; `make test-native-image`
runs it as a native executable. The contract decodes changing, empty, repetitive,
and incompressible inputs and compares the full output with GZIPOutputStream.
It also verifies reuse after large output growth and rejects use after close.

## Alternative backends

Run `make bench-compression`, or `python3 tools/compression_probe.py --iterations 2000`
for a shorter probe. Cargo builds each backend separately from a locked,
non-published workspace under `benchmarks/compression`. No compression dependency
is added to Bemo's published artifacts. Compilation precedes measurement; avoid
running builds or other benchmarks concurrently.

The probe compares level-6 raw DEFLATE with identical gzip framing and CRC32,
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

The probe pins flate2 1.1.10; its lockfile pins zlib-rs 0.6.8 and the native
zlib-ng dependency. The zlib backend uses the host's zlib. zlib-ng is the first
candidate for a subsequent provider integration on this host, with zlib-rs close
on large repetitive input. Both alternatives produce a larger result for that
input: CPU comparisons alone do not establish a full-stack win. Repeat on the
dedicated Linux benchmark runner before selecting a shipped backend.

[zlib-rs](https://github.com/trifectatechfoundation/zlib-rs) offers Rust and
zlib-compatible C APIs; [zlib-ng](https://github.com/zlib-ng/zlib-ng) offers a
native API and zlib compatibility mode. Switching this isolated Rust probe does
not switch Java's Deflater or GraalVM's linked zip implementation. Provider
integration must be explicit and separately qualified through both bindings.

The early local end-to-end before/after samples overlapped Native Image builds
and are unsuitable for performance claims. CodSpeed CI must establish the
application-level improvement of state reuse separately from backend selection.
