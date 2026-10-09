# Recent exact-capacity buffer reuse

This change is stacked on [initialized TLS output reuse, PR #17](https://github.com/elide-dev/bemo/pull/17).
The preceding CodSpeed investigation found buffer reuse and admission together
accounted for about 30% of the one-byte-fragmented persistent GET profile.
The [source run](https://app.codspeed.io/elide-dev/bemo/runs/6ac7daee19d1bceb3f5097ea)
predates both changes; its CPU shares motivate investigation rather than
establishing the resulting speedup.

Exact-capacity ordinary and initialized TLS allocations now search backwards
and stop at the first matching pool entry. The most recently returned matching
allocation is normally last, so the common repeated geometry avoids a full
smallest-fit scan and avoids moving a different entry during removal. This is a
recency heuristic: interleaved receive selection or trimming can reorder entries
through `swap_remove`. Entries
retain their existing type, exact-capacity matching, pool bounds, and budget
reservation rules. Received buffers still select the smallest sufficient
capacity belonging to their owner. Retained leases still prevent admission.
The public Rust API, C ABI, Java bindings, and dependencies are unchanged.

Tests check ordinary and initialized TLS recency, receive smallest-fit selection
with a smaller allocation from another owner, and the existing ownership,
closure, retention, and cross-thread release contracts. The new ordinary recency
test failed against the previous selector and passes with the change.

## Local measurements

[Raw evidence](data/recent-pool-reuse-local-20261008.json) retains every Criterion
sample, slope estimates, source and binary hashes, base commit, CPU, toolchain,
and sample order. The accompanying [before source patch](data/recent-pool-reuse-before-source.patch)
and [after source patch](data/recent-pool-reuse-after-source.patch) reproduce the
recorded buffer source hashes when applied independently to the parent commit.
Both variants include the new test-only TLS recency test; the final optimized
source differs from its measured snapshot only by formatter changes.
Both variants were built before timing. Three repetitions
alternated before/after order; each used 30 samples, one second of warmup, and
two seconds of measurement on macOS ARM64 without CPU affinity.

The new pool benchmarks seed 1, 8, or 16 mixed exact-capacity allocations,
ending with a 4 KiB allocation. They measure repeated allocation and release
of that last geometry; pool preparation and trimming are outside measurement.
These exercise a favorable steady-state recency pattern rather than random
geometry or receive-buffer best-fit selection. HTTP cases use the existing
persistent GET benchmark, including input-buffer creation, parser work,
event storage, and cleanup.

| Workload | Before median | After median | Time reduction |
| --- | ---: | ---: | ---: |
| Exact pool, 1 entry | 12.821 ns | 12.857 ns | -0.28% |
| Exact pool, 8 entries | 13.072 ns | 12.867 ns | 1.57% |
| Exact pool, 16 entries | 16.860 ns | 13.131 ns | 22.12% |
| Complete persistent GET | 80.486 ns | 80.273 ns | 0.26% |
| GET in 7-byte fragments | 293.043 ns | 286.573 ns | 2.21% |
| GET in 1-byte fragments | 1376.587 ns | 1304.313 ns | 5.25% |

Each median is the median of three repetition slope estimates. Time reduction
is `100 * (1 - after / before)`. The single-entry case and complete GET are
effectively unchanged at this measurement scale. The 16-entry case is faster
in every paired repetition, as is the one-byte GET. Seven-byte GET is slower
in one pair. These local operation timings do not establish Linux server
throughput, latency, or CPU-per-request improvement.

The new pool benchmarks participate in CodSpeed's existing buffers simulation
suite. Compare the stacked branch directly against PR #17 so TLS output reuse
does not enter this change's delta. Check complete and fragmented HTTP, ordinary
allocation, initialized TLS encryption, and ownership benchmarks on matching
runners. Preserve receive selection and inspect unexpected regressions before
merging.

## Validation

On macOS ARM64, `make fmt`, `make build`, `make check`, `make test`,
`make test-native-image`, and `python3 tools/test_git_dependency.py` passed.
The full suite includes 335 Rust tests and the Java/shared ABI contracts.
Local Java runs used `JAVA_TOOL_OPTIONS=-Djdk.lang.Process.launchMechanism=FORK`
to work around this installation's failing GraalVM process-spawn helper.
No runtime default was changed.

`MIRIFLAGS=-Zmiri-disable-isolation cargo miri test -p bemo --locked
--no-default-features --features rust-allocator --lib --test pool` also passed:
131 library tests and 16 pool tests, with 24 existing library exclusions for
unsupported runtime/native operations. The isolation flag matches CI.
Independent code review found no remaining correctness or provenance issues.
