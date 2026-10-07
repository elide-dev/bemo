# Five-PR performance optimization stack

## Objective and delivery

Implement the five optimization opportunities identified using CodSpeed MCP,
as five successive pull requests. PR 1 targets current `origin/main`; each
subsequent PR targets the preceding PR's branch. Each PR must stand on its own
base, contain its own correctness and measurement evidence, and be left open
for review. Opening PRs is authorized; merging and publishing are outside this
request.

The ranking comes from 73 measured benchmarks at main commit `499a6f3`.
Another 65 historical benchmarks were skipped. Flamegraph percentages describe
profile costs, not promised application speedups. In particular, wall-time
profiles include startup, warmup, clients, and multiple samples; simulation
excludes syscall costs. Preserve raw evidence and compare equivalent workloads.

## Shared requirements

- Preserve ABI 3 symbols, layouts, handle ownership, and initialized foreign
  storage. Use existing APIs wherever possible.
- Keep Rust transport state independent of JVM, Netty, GraalVM, and Elide types.
- Java compilation, dependency resolution, formatting, and Native Image builds
  remain Elide-owned. Do not introduce a separate Java build system.
- Preserve workload closure, owner budget admission, cross-thread release,
  callback reentry, and kernel retirement rules.
- Dynamic gzip is performed for every response on both compared stacks. Do not
  replace this workload with precompressed constant output.
- Keep existing copy microbenchmarks; add benchmarks for the new paths instead
  of removing work from the operations the old benchmarks promise to measure.
- Keep benchmark fingerprints strict. A changed application path establishes
  a new workload fingerprint and requires matched Bemo/Netty measurements.
- Run `make fmt`, `make build`, `make check`, `make test`,
  `make test-native-image`, and `tools/test_git_dependency.py` as appropriate
  to each PR. Ownership changes also require the relevant Miri/fuzz/sanitizer
  checks. Linux-specific allocator and driver qualification comes from CI.
- Engineering documentation lives under `docs/`; no local absolute paths or
  credentials are committed.

## PR 1: Reuse dynamic gzip compression state

### Design

Replace per-request `GZIPOutputStream` construction in
`benchmarks/java/NativeHttpBenchmarkServer.java` with a reusable compressor
owned by the benchmark server's event thread. Extract its implementation into
a small Java helper under `benchmarks/java` so it can be verified independently.
Use a raw DEFLATE stream with the gzip header, CRC32, and uncompressed-size
trailer, resetting stream and checksum state between responses. Close the
compressor during server teardown. Reuse bounded output storage and avoid
`ByteArrayOutputStream.toByteArray()`'s extra output allocation.

Keep the current compression level and per-response compression semantics for
the first change. Alternative compression levels and static response caching
are excluded so the comparison isolates reuse. An isolated non-published Cargo
probe compares zlib, zlib-rs, and zlib-ng in response to the user's follow-up;
it does not change the JVM's compression provider or published dependencies.
Deflater reset still has a cost; do not claim that reuse eliminates all reset
work shown in the profile.

### Acceptance

Decode successive outputs with a standard gzip decoder and compare exact bytes.
Cover empty content, 1 KiB and 64 KiB payloads, incompressible content, changing
payloads, output growth, and teardown. Check repeated use does not concatenate
members accidentally or reuse an earlier CRC/size. Measure dynamic gzip against
the previous implementation for both sizes and retain output-size evidence.

## PR 2: Retain immutable response body storage

### Design

Use the existing `httpRespond` streaming flag and add `CHUNK_RETAIN` to
`httpChunkSend`. The body is a frozen handle whose initialized bytes begin at
offset zero. The caller keeps the handle on every result; successful submission
creates an independent driver lease. HTTP/1 declared-length/close framing and
HTTP/2 support retained parts; HTTP/1 chunked framing still requires prepared
mutable storage. No C symbol, ABI version, or layout changes.

Identity bodies are uploaded once and reused per response. Dynamic gzip still
compresses every response and copies the result once into frozen native storage,
then queues the head and body separately. Borrowed-pointer `httpRespond` keeps
its existing synchronous copy contract. Both Java bindings forward the new
flag and run the same wire and lifetime contract.

Storage cannot return to the allocator while a queued part or kernel operation
still holds a lease. Freeing an exchange after its final part suppresses
undelivered notifications but leaves remaining parts on the wire. Driver
retirement can remain busy while cancellation completes, especially on io_uring.

### Acceptance

Run matched plaintext/TLS, identity/gzip, 1 KiB/64 KiB workloads through FFM and
Native Image. Exercise HEAD, empty bodies, multiple pipelined requests,
backpressure, disconnected peers, failed transfer, and early exchange release.
Verify response order, exact body content, gzip decoding, completion reporting,
and zero live owner charges at teardown. Add an owned header/body benchmark
beside the existing contiguous response encoding benchmark.

## PR 3: Extend bounded reuse to ordinary buffer allocations

### Design

Extend the existing buffer reuse machinery to ordinary `Buffer::new`
allocations, rather than introducing an independent allocator or global pool.
Keep generic allocation geometry exact: a generic buffer requesting N bytes
must reuse only an N-byte backing allocation. Receive storage may retain its
existing best-fit behavior.

Reuse storage only after the last allocation owner, frozen view, lease, and
kernel operation has retired. Idle pool entries release live budget charges;
reuse re-reserves capacity and rechecks closure. Keep thread-local retention
bounded by 16 entries and 256 KiB. For generic storage, the owner's retention
allowance is at most half its limit, allowing one 4 KiB reusable block for the
8 KiB owners in the ABI benchmark; receive-only retention retains its existing
one-eighth allowance. Both classes share the thread's total bound.

`Buffer::new` still returns logical length zero. Foreign-accessible allocations
still initialize all exposed capacity before exposing addresses, even when
pooled bytes belonged to an earlier request. Provided receives with return
slots retain their dedicated return path. Closed owners do not admit new pool
entries or allocations; trimming and thread teardown free idle entries.

Retain the backing storage and its Arc descriptor through an allocation-lease
wrapper. Only the final lease may admit an exclusively owned descriptor; idle
entries are uncharged, and reuse restores their charge before exposure.
Ordinary exact entries can change budget owners after all old leases end;
receive entries keep owner affinity. Provided return slots cannot enter this
pool. Miri and lifetime/accounting contracts verify the descriptor reuse.

### Acceptance

Test exact capacity, admission at budget limits, retained slices delaying reuse,
cross-thread final release, closure races, bounded idle memory, trimming, and
thread teardown. Verify reused foreign-visible bytes are zero, including when
different payloads previously filled them. Run Miri and buffer/ABI fuzz checks.
Measure repeated ordinary allocate/drop and ABI allocate/release, documenting
live charges separately from retained pool memory.

## PR 4: Reduce reparsing and growth for fragmented HTTP heads

### Design

Cache validated request-line metadata, complete header spans, and a parse cursor
while accumulating a fragmented head. Completed lines are processed once;
the incomplete current line still goes through the existing strict parser on
every fragment, preserving early rejection without adding a second validator.
Reserve at least 128 accumulator bytes for common fragmented heads. Contiguous
requests keep temporary progress on the stack.

Delivered exchanges own their header spans; progress resets on completion and
error. Body and trailer framing keep their separate existing paths and bounds.
This avoids reprocessing complete prefixes and rebuilding their header vectors;
very long individual incomplete lines may still require repeated scanning.

### Acceptance

Assert fragmentation invariance for every split of representative valid and
invalid heads, including terminators split across buffers, pipelined input,
oversized heads, conflicting lengths, transfer encoding, trailers, and closure.
Include one-byte fragments and preserve existing framing/security regressions.
Run parser fuzzing. Compare contiguous, seven-byte, and one-byte inputs, and
include persistent-connection measurements so parser-construction costs are
visible separately.

## PR 5: Improve integer registry hash distribution

### Design

Improve buffer-registry fingerprint distribution for internally minted numeric
handles, preserving bucket locality, keys, domain routing, identities, locks,
and reentry rules. Keep owner identity hashing: measured lookups did not benefit
from mixing. Identity hashing leaves sequential handles with shared high
fingerprint bits. Use a cheap deterministic multiplicative fingerprint mixer in buffer registries;
a full avalanche mixer loses sequential lookup locality. Scope replacement to
maps verified to use internal numeric keys; keep untrusted string hashing
unchanged.

This is the first concrete improvement to handle bookkeeping. A generational
slot table is excluded from this stack because changing handle minting and
lookup is substantially broader than necessary to address the observed map
cost. Do not claim reduced lock contention from the current single-threaded
benchmark.

### Acceptance

Verify bucket and fingerprint distribution for sequential handles and domain
prefixes. Preserve forged/stale handle rejection, repeated releases, domain
reuse, owner closure, and cross-thread release. Measure lookup, insert/remove,
and slice/release at 1, 64, and 4096 live entries against the previous hasher.
Run ABI contracts, Miri, and ABI fuzzing. If the mixer regresses small maps,
investigate before publishing a performance claim.

## Stack verification and review

Each PR description names its immediate base, concrete behavior change,
correctness checks, measured results, and platform limits. Later PRs must show
only their delta against the preceding branch. Re-run affected comparisons after
each layer so gains are attributable; review the final cumulative diff for
ownership interactions between compression, streaming, and pooling.

Open draft PRs while platform qualification is pending. Mark checks passed only
after observing their output, and report unavailable Linux checks explicitly.
Do not merge the stack or publish artifacts automatically.
