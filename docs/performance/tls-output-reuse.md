# Initialized TLS output reuse

The [October 8 CodSpeed run](https://app.codspeed.io/elide-dev/bemo/runs/6ac7daee19d1bceb3f5097ea)
at `0eca65f` attributed 21.24% of the TLS 1.3 established 128 KiB encryption
profile to copying and 6.42% to zero initialization. Its server-only native HTTP
profile attributed 19.02% to copying and 5.83% to zero initialization, but the
largest server hotspot remained an unnamed symbol. These profiles precede this
change and do not establish its performance impact.

TLS output now uses a private wrapper whose full capacity is initialized once.
Only fully initialized allocations enter its separate pool category. Subsequent
encoding calls overwrite that storage without another zero pass, and freezing
exposes only the encoder's reported output length. Retained ciphertext views
prevent recycling until the final lease is released. Budget reservation, pool
bounds, and terminal-session behavior remain in place.

Recovering a frozen output as an ordinary mutable buffer removes its initialized
pool category before exposing `MaybeUninit` access. Foreign buffer allocation
continues to zero capacity; TLS output storage cannot be reused by that path.
Tests cover initialized recycling, shortened output, retained views, exhausted
and closed budgets, and uninitialized writes after mutable recovery. The storage
tests also pass under Miri.

Local validation passed `make build`, `make check`, `make test`,
`make test-native-image`, `make bench-prepare`, and
`python3 tools/test_git_dependency.py`. The full Rust suite passed all 332 tests.
The matching Rust LLVM symbol reader confirmed named Bemo session symbols in
the optimized archive. The local JDK could not spawn OpenSSL through its default
process helper; a standalone Java probe reproduced the failure, and validation
used `JAVA_TOOL_OPTIONS=-Djdk.lang.Process.launchMechanism=FORK`. This workaround
is not part of the repository's runtime configuration.

## Local timing evidence

[Raw measurements](data/tls-output-local-20261008.json) retain Criterion samples,
slope estimates, binary and source hashes, the base commit, toolchain, CPU, and
sampling settings. Before and after binaries were built before timing and run
in alternating order for three repetitions. Each repetition used 30 samples,
one second of warmup, and two seconds of measurement on macOS ARM64, without
CPU affinity. The workload was TLS 1.3 established encryption of 128 KiB.

| Variant | Median repetition slope | Repetition point-estimate range |
| --- | ---: | ---: |
| Before | 18.014 microseconds | 17.995–18.024 microseconds |
| After | 17.441 microseconds | 17.408–18.621 microseconds |

The ratio of these medians is a 3.18% time reduction. One after repetition was
slower, and the observed ranges overlap; this is local qualification rather
than a statistically established Linux transport gain. A preceding isolated
probe reported a 2.7% regression and prompted the alternating repetitions.
That probe is excluded from the table, which contains only the paired schedule.
The later mutable-recovery guard refinement is outside the measured encryption
path; the raw source fingerprints identify the exact measured variant.

## Next Linux profile

Benchmark preparation now retains optimized Rust debug information and, on
Linux, Native Image debug information and local method symbols. It keeps `-O3`
and does not enable `SourceLevelDebug` or change frame-pointer generation.
GraalVM documents these options in its
[debug information reference](https://www.graalvm.org/jdk25/reference-manual/native-image/debugging-and-diagnostics/DebugInfo/).
Publishing builds keep their existing release configuration.

The next CodSpeed CI run should qualify established encryption across sizes and
TLS versions, then re-examine the large-response server profile with server PID
filters. Verify symbol resolution rather than assuming the unnamed hotspot is
encryption. Compare matched runners and use the paired transport workflow to
measure server CPU per request, throughput, and p99. Rustls payload copies remain
the next candidate after that attribution; this change only removes repeated
TLS output initialization.
