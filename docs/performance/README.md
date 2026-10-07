# Reproducible performance charts

The README's SVGs come from `tools/plot_bench.py`, with Matplotlib and its
dependencies pinned in `tools/chart-requirements.txt`. The default input is
`data/provenance.json`, which names the raw benchmark summary beside it.
Every SVG embeds the source SHA-256 and evidence reference. Generation has no timestamps
or random chart IDs; identical input and rendering dependencies reproduce the
same SVG bytes. Fonts are embedded as paths for consistent browser rendering.
Chart generation requires Python 3.12+; the core build still supports Python 3.11+.

```sh
make bench-graphs
```

This creates an isolated environment under `build/chart-venv` and renders the
four checked-in SVGs: three basic transport charts and the Spring Boot/Micronaut
framework matrix. The framework chart derives medians and sample ranges from
`data/framework-unclemax-level1.json`, with all five endpoints in JVM and Native
Image modes. Each filename includes the first 12 characters of the
source data SHA-256, so GitHub image redirects cannot reuse an older chart URL.
When the evidence changes, update the affected README image paths to the generated
filenames and remove the superseded SVGs. Query strings on relative image links
do not survive GitHub’s redirect to raw content.
It does not run benchmarks or upload metrics. PNG export
and alternate destinations are available:

```sh
make bench-graphs CHART_ARGS='--output build/chart-preview --png'
```

## Refresh from a benchmark

Prepare and run the existing complete matrix, with at least three samples:

```sh
make bench-prepare
make bench-transport
cp build/reports/benchmarks/summary-all.json docs/performance/data/linux-x86_64.json
```

Alternatively, download `transport-evidence` from a completed CI run:

```sh
gh run download <run-id> --repo elide-dev/bemo --name transport-evidence --dir build/readme-bench/latest
cp build/readme-bench/latest/summary-all.json docs/performance/data/linux-x86_64.json
```

Update `data/provenance.json` with the actual date, full measured commit, run URL,
artifact name, runner description, and summary filename. For local measurements,
use a checked-in evidence reference and identify the host accurately. Record a
working-tree snapshot and per-file hashes when the measured source is uncommitted;
the parent commit alone does not identify it. Set `matched_compression: true` and `matched_tls: true` for
current evidence so charts reject compression-level, TLS protocol, and cipher mismatches.
Then run `make bench-graphs` and review the data and images together. Keep raw
samples: medians, sample ranges, CPU costs, and memory maxima are computed from
them, rather than copied from summary aggregates or written into plotting code.

The renderer refuses incomplete matrices, fewer than three samples, mismatched
commits/environments, unmatched compression levels in current evidence, backend fallbacks, mixed drivers, unexpected runtime/codec
stacks, and invalid metrics. It currently requires Linux RSS measurements and
all workloads in either the archived eight-case or extended twelve-case matrix
with the same sampling/connection settings. The one-sample
per-command CodSpeed wall-time summaries are not suitable for these charts.
For a dedicated runner, execute the full paired matrix there and retain its
summary and environment evidence. Do not combine samples from different hosts
or present hosted-runner variation as a statistically established win.

## Compare two commits on one runner

For an optimization before/after comparison, use the dedicated dispatch workflow:

```sh
gh workflow run on.bench-paired.yml --ref main \
  -f before=<pre-optimization-commit> -f after=<merged-commit>
```

It builds both versions before any timing, uses separate worktrees and prepared
artifacts, then alternates commit order across workload/sample pairs and
Bemo/Netty launch order across samples. The complete matrix takes roughly half
an hour or more; the workflow allows 90 minutes. It uploads
`paired-transport-evidence`, containing both complete summaries, individual raw
samples, logs, resolved commits, CPU details, and allowed affinity. Its token
has read-only repository permissions and it runs no publishing jobs. Separate
jobs run the shared Rust/JVM/Native Image and safety contracts. The transport
host first compares the pinned zlib-rs levels 1/3/6 through 128 KiB;
`compression-evidence` records compression and checksum measurements.

A separate host qualifies retain/drop, slice/drop, and exclusive recovery against
the pre-stack plain-Arc baseline (`499a6f3`). It prepares both versions before
three alternating ownership samples, each using 30 Criterion measurements after
one second of warmup and two seconds of measurement. `lease-evidence` includes
raw logs, parsed nanosecond estimates, commits, CPU, and affinity. This addresses
the earlier cross-CPU simulation comparison without changing ownership merely
to improve an instrumented estimate.

The underlying Linux command is:

```sh
python3 tools/bench_pair.py --before <commit> --after <commit>
```

Use the `after/summary-all.json` for the refreshed charts and retain the before
summary and the shared `environment.json` beside it. Each chart provenance
should identify the same paired run and the actual measured commit. The
workload-source fingerprint includes the native server, so server optimization
changes its hash; verify that the client, requested workload, measurement
function, runtime, and provider settings remain comparable before calculating
a cross-version delta. [The scaling results](scaling-results.md) retain
this evidence and all twelve workloads;
[the earlier stack results](optimization-results.md) remain archived.

## Reading the charts

- Throughput dots are sample medians; whiskers span the minimum and maximum of
  the samples, not confidence intervals. Percentages divide the two medians.
- Payload curves show only the measured 1 KiB, 64 KiB, and 128 KiB identity bodies.
  Lines connect the measured points without fitting or inventing intermediate data.
- Server CPU is the median measured server CPU time per completed request.
- Memory is the maximum sample sum of server/client lifetime `VmHWM`, including
  startup and warmup. It is not a simultaneous peak, server-only RSS, or heap size.

The current snapshot uses Bemo Native Image `-O3`, native HTTP, Rustls/aws-lc-rs,
and io_uring against stock OpenJDK Netty epoll/JDK TLS. Four persistent clients
use the same external OpenJDK Netty NIO load generator. Each server has one I/O
thread; server/client heaps are 256 MiB each. Each sample warms 5,000 rounds,
then measures 25,000 rounds per client, completing 100,000 requests. Gzip uses
per-response application compression through native zlib-rs level 1 for Bemo
and the Netty compressor explicitly set to level 1 for the comparator, with compressible fixed-pattern bodies.
These compare the complete stacks, including their different HTTP/TLS implementations.
See [measurement methodology](../measurement.md) for timing, ownership,
backend selection, and environment controls.

The paired helper extends older CLI case registries in memory for 128 KiB.
The older server and client are rebuilt unchanged; their variable-size workload
already supports that payload. Sampling logic and the requested size stay
identical across versions. The renderer supports archived eight-case data and
requires every case/transport when the extended twelve-case matrix is present.

Current TLS clients are pinned to TLS 1.3 / `TLS_AES_128_GCM_SHA256`. The basic
JDK client checks every negotiated session. The framework runner re-executes
with the checked-in OpenSSL policy before initialization, checks HTTPS readiness,
and enforces the same policy at wrk context creation using a benchmark-only
preload shim. `make bench-prepare` builds that Linux shim and requires OpenSSL
development headers and libraries. The shim is applied only to wrk, never to
application servers. Positive/negative cipher controls verify the actual client.
