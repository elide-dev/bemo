# Initial framework benchmarks on Unclemax

Baseline note: these archived framework measurements used Netty NIO and JDK
TLS. The current harness requires native epoll/kqueue and tcnative/BoringSSL.
These numbers must not be presented as results against that native baseline;
see the [native epoll/tcnative cycle](native-baseline.md) for the corrected baseline.

Measured on October 7, 2026, using the Spring Boot and Micronaut examples in
this working tree. The build snapshot is based on
`a8bf91d49ea7719ab8a4460aa22f8d97bf93ae51` and includes uncommitted example
sources. Its per-file hashes are retained with the evidence; the parent commit
alone does not identify the measured implementation.

## Results

Median throughput over three samples per case:

| Framework | Runtime | Bemo req/s | Stock Netty NIO req/s | Bemo change |
| --- | --- | ---: | ---: | ---: |
| Spring Boot | JVM | 158,030 | 153,193 | +3.2% |
| Spring Boot | Native Image | 124,900 | 119,065 | +4.9% |
| Micronaut | JVM | 250,546 | 254,798 | -1.7% |
| Micronaut | Native Image | 200,539 | 188,253 | +6.5% |

Both native comparisons favor Bemo in all three paired repetitions, with
non-overlapping throughput ranges. JVM ranges overlap: this initial round
does not establish a statistically reliable JVM win or loss. Micronaut JVM
also reverses direction in the third pair.

| Framework / runtime | Bemo sample range, req/s | Stock sample range, req/s |
| --- | ---: | ---: |
| Spring Boot / JVM | 154,028–158,805 | 152,673–154,690 |
| Spring Boot / Native Image | 124,894–125,715 | 118,729–119,083 |
| Micronaut / JVM | 248,038–251,703 | 250,719–260,281 |
| Micronaut / Native Image | 199,380–200,540 | 187,760–190,154 |

Latency, server RSS, and time to the first verified response are also sample
medians. Native startup values are limited by the 50 ms readiness polling
interval. RSS is resident server memory after measurement, not allocated heap
or a server/client sum.

| Framework | Runtime | Transport | p50 / p99, µs | Server RSS, MiB | Verified response, ms |
| --- | --- | --- | ---: | ---: | ---: |
| Spring Boot | JVM | Bemo | 402 / 442 | 380.0 | 898 |
| Spring Boot | JVM | Stock NIO | 405 / 796 | 395.3 | 823 |
| Spring Boot | Native Image | Bemo | 491 / 1,325 | 113.9 | ≈50 |
| Spring Boot | Native Image | Stock NIO | 518 / 1,134 | 106.8 | ≈50 |
| Micronaut | JVM | Bemo | 253 / 272 | 328.9 | 638 |
| Micronaut | JVM | Stock NIO | 249 / 265 | 326.7 | 494 |
| Micronaut | Native Image | Bemo | 312 / 715 | 82.0 | ≈50 |
| Micronaut | Native Image | Stock NIO | 333 / 721 | 77.7 | ≈50 |

JVM throughput exceeds native throughput in all four matching combinations.
Native mode has substantially lower resident memory and time to the first
verified response under these settings. Bemo's native throughput gains do not
improve every tail-latency result: Spring native p99 is higher with Bemo in
this round. The GC and runtime differences below are part of the comparison.

All 24 measured samples completed: 87,306,363 validated responses, zero client
socket/status/body errors or timeouts, and no logged AUTO fallback. A negative
test returned HTTP 200 with an incorrect 13-byte body; the runner rejected all
25 completed replies, confirming it does not accept status alone.

The [checked-in dataset](data/framework-unclemax-20261007.json) contains all
individual metrics, summaries, source/artifact/harness hashes, and backend
probe results. Full build, server, wrk, and strace logs are retained locally in
[`build/reports/unclemax-framework-20261007`](../../build/reports/unclemax-framework-20261007).

## Method

- Host: Unclemax, Linux x86-64, AMD Ryzen Threadripper PRO 9965WX, 24 physical
  cores / 48 hardware threads, Linux `7.0.0-34-generic`.
- CPU governor and energy preference: `performance`, with boost and kernel
  mitigations retained. No host tuning or background services were changed.
- Server affinity: physical cores 12–17, in one L3 group. Client affinity:
  physical cores 18–21, in another L3 group; no shared SMT siblings.
- Builds: pinned Elide `1.5.4+20260921`, Cargo release artifacts, Native Image
  `-O3`, `x86-64-v3`, Oracle GraalVM `25.3.4.1+1.1`, Java `25.0.4.1+1-LTS`.
  Native Image reports ML-inferred PGO; no workload-trained profile was supplied.
  JVM measurements use stock OpenJDK `25.0.2` with its default G1 collector;
  native executables use Serial GC.
- Frameworks: Spring Boot `4.1.1`, Micronaut `4.10.30`, Netty `4.2.18.Final`.
  Both retain their framework HTTP routing and codecs. Bemo replaces socket
  transport, using FFM on the JVM and static CAPI linkage in native mode.
- Bemo AUTO selected io_uring in all four separately traced startup probes;
  measured process logs retain any AUTO fallback diagnostics. Stock transport
  uses Netty NIO with the Linux JDK epoll selector; the resolved classpaths
  contain no Netty native epoll JNI artifact.
- Two server workers; Micronaut also has a separate acceptor. The Spring Bemo
  example shares workers with its acceptor. JVM and native processes receive
  `-Xms256m -Xmx256m`; RSS includes memory outside that heap.
- wrk `debian/4.1.0-4build3`, four threads, 64 persistent HTTP/1.1 connections,
  no pipelining, loopback `GET /plaintext`, exact 13-byte `Hello, World!` body.
  A Lua response hook validates every completed status/body; startup also
  checks content type and reuse of the same connection. The hook adds client
  work. wrk reports latency in microseconds, per its
  [scripting API](https://github.com/wg/wrk/blob/master/SCRIPTING).
- Three fresh processes per framework/runtime/transport. Each gets 20 seconds
  of warmup and 20 seconds of measured load. Pair order rotates and transport
  order reverses between repetitions. Builds, correctness tests, and strace
  probes finish before timed measurements.

## Scope and caveats

This is an initial closed-loop plaintext result at one concurrency. It does
not establish peak capacity, performance under an open-loop arrival process,
TLS/compression behavior, or application performance with databases and JSON.
The client shares the host with the server. Background Java processes remained
running at low CPU use; the machine was not exclusively reserved.

Native Bemo and stock cases use the same executable with a runtime transport
switch. The stock native image therefore still contains compiled Bemo code.
Framework/runtime comparisons include differing HTTP stacks, loop topology,
JIT versus ahead-of-time compilation, and different Java distributions.

Server CPU deltas bracket the measured wrk subprocess, including its startup
and teardown; wrk throughput uses its own timed interval. RSS is sampled after
measurement and lifetime high-water RSS is retained in the raw samples.
Startup is wall time to the first verified HTTP reply using readiness polling,
with warm file caches and a fixed heap; it is not a cold-start measurement.

Bemo server logs contain connection-reset exceptions when wrk closes its
persistent sockets at warmup/measurement boundaries. These teardown logs are
preserved separately from client error counters. Reset classification/logging
in framework integration warrants follow-up even when replies validate.

## Reproduction

See [framework setup and benchmark commands](../framework-examples.md#benchmarking).
Use disjoint CPU sets suitable for the target host. The runner writes a new
output directory containing per-sample JSON, raw server/wrk logs, environment
and artifact hashes, and summary statistics. Separate `--probe` runs retain
syscall evidence without perturbing timed runs.
