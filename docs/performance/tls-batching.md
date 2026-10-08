# TLS batching and borrowed vectored writes

The focused October 8, 2026 run improved Bemo's large TLS throughput to roughly
parity with Netty epoll and tcnative/BoringSSL on unclemax. This measures three
selected workloads before and after the optimization; it does not replace the
[full framework/native baseline](native-baseline.md).

| Workload | Bemo before, RPS | Bemo after, RPS | Netty after, RPS | Bemo gain |
| --- | ---: | ---: | ---: | ---: |
| JVM Netty HTTP, identity, 128 KiB | 27,032 | 29,264 | 33,675 | +8.3% |
| Native HTTP, TLS identity, 64 KiB | 22,477 | 26,476 | 26,588 | +17.8% |
| Native HTTP, TLS identity, 128 KiB | 13,753 | 15,440 | 15,517 | +12.3% |

These are medians of three repetitions. Bemo's observed RPS ranges were
26,961–27,832 before versus 29,049–29,297 after for the JVM payload,
22,396–22,683 versus 25,777–26,504 for 64 KiB TLS, and
13,524–13,806 versus 15,312–15,470 for 128 KiB TLS. Ranges are observed sample
extrema, not confidence intervals; the sub-percent TLS difference from Netty
should be read as near parity.

Bemo server CPU fell from 29.0 to 20.0 microseconds per request for 64 KiB TLS,
and from 50.0 to 31.5 for 128 KiB TLS. Netty's corresponding after-run medians
were 22.3 and 36.5 microseconds. Bemo TLS p99 fell from 180.8 to 152.6 microseconds
at 64 KiB and from 297.6 to 267.3 at 128 KiB, close to Netty's 153.7 and 264.4.
The JVM plaintext path still trails Netty: 29.2 versus 24.0 microseconds of
server CPU, and about 13.1% lower throughput. Framework throughput has not
been remeasured in this focused run.

The separate warmed JVM encryption harness measured 64 KiB medians of
3.790 microseconds for NativeSslEngine/FFM/Rustls/AWS-LC and 4.588 for
Netty/tcnative/BoringSSL. At 128 KiB they were 7.536 and 7.287 microseconds.
It includes FFM/JNI overhead and times only server wraps. The common JSSE
peer decrypts and validates outside that timer. It is not an isolated crypto
primitive comparison, and these unpinned microbenchmarks do not share the
macro workload's affinity controls. NativeSslEngine used one/two wraps per
64/128 KiB response; the stock engine used four/eight.

## Changes and ownership

Native session writes now accept up to 128 KiB per transition. Rustls fragments
the application bytes into legal TLS records inside one pooled ciphertext
allocation. Allocation admission failure reduces batching toward the previous
16 KiB behavior rather than requiring a larger owner budget. The HTTP lane
preserves handshake acknowledgements and retains response parts until every
ciphertext byte completes, including partial sends and cancellation.

Unix sockets can attempt a synchronous nonblocking send before owned submission,
including sockets attached to io_uring. The new additive ABI operation borrows
up to 64 direct regions and 128 KiB total for one vectored syscall; it retains
no addresses or operations after return. Both Java bindings preserve source
geometry and lifetime. Netty borrows eligible exclusive direct messages;
backpressure, shared buffers, and frozen native handles retain the existing
owned asynchronous paths. io_uring remains the selected receive backend.

The benchmark changes add 128 KiB CodSpeed commands, JVM Netty-codec commands,
established-session Rust encryption, and a matched JVM encryption harness.
The copying HTTP microbenchmark no longer includes an asymmetric payload scan,
so its historical CPU values are not a compatible baseline. The retained
response implementation and existing allocation pool remain in use.

## Controls and evidence

The macro run used the same host, stock JDK 25.0.2 client, four persistent
connections, 5,000 warmup rounds and 25,000 measured rounds per connection,
256 MiB heaps, and CPU groups 12–17 for servers and 18–21 for clients. Both
variant order and transport order reverse on odd repetitions. All 3.6 million
responses validated. Bemo explicitly requested io_uring (backend 2); the after
readiness report confirms io_uring with no fallback. The native HTTP comparisons
retain different server runtimes, as in the earlier baseline; the JVM
Netty-codec comparison uses the same runtime and HTTP codec on both sides.
The original before-run prepared artifacts were preserved. The HTTP payload and timed load generator are unchanged. The after-run JVM
server readiness report resolves its actual backend and fallback state; the
before-run JVM server is explicitly configured for io_uring despite its older
transport-only label. Each preparation's source and artifact hashes are retained
in the evidence.

[Raw samples, summaries, artifact fingerprints, and encryption measurements](data/tls-batching-20261008.json)
include the before-run source manifest and the after-run source fingerprints.
Subsequent changes add readiness unit tests in `tools/test_bench.py` (its measured
and PR hashes are recorded separately) and bound the Rust established-session
microbenchmark to 4,096 operations per fresh session. The latter avoids TLS
record-counter exhaustion during long Criterion warmups; handshakes remain
outside its encryption timer. Neither change alters the measured macro servers
or timed load generator.
Validation passed on macOS/kqueue and Linux/io_uring and epoll: `make build`,
`make check`, `make test`, `make test-native-image`, `make bench-smoke`, and
`python3 tools/test_git_dependency.py`. Both FFM and Native Image contracts cover
borrowed vector ordering, unchanged buffer geometry, admission rejection,
source mutation after return, and no retained native lease. Rust tests exercise
partial writes, backpressure, owned fallback ordering, legal multi-record
encoding, ciphertext lifetime, and small owner budgets.

Reproduce the benchmark families with `make bench-prepare`,
`make bench-tls-records`, and the selected cases of `tools/bench.py run`.
See [measurement instructions](../measurement.md) for timing scope and controls.
