# Native safety verification

The ordinary contract suite exercises the production allocator, native drivers,
TLS, HTTP, ownership, and both JVM bindings. Sanitizers, Miri, and fuzzing add
evidence that the suite alone cannot provide:

```sh
rustup component add rust-src miri
cargo install cargo-fuzz --version 0.13.1 --locked
make test-asan
make test-tsan
make test-miri
make fuzz-smoke
make fuzz
```

## Sanitizers through the Elide toolchain

`make test-asan` and `make test-tsan` use the pinned
[Elide toolchain](https://github.com/elide-dev/toolchain) from
`tools/versions.json`, and its checksum-pinned sanitizer add-on where one exists
(Linux x86-64). `tools/verify.py` installs both under `build/tools/` on first
use. One clang sanitizer runtime serves every instrumented component:

| Component | Instrumented by |
| --- | --- |
| Bemo and its Rust dependencies, Rust std (`-Zbuild-std`) | `-Zsanitizer=…` with `-Zexternal-clangrt` |
| AWS-LC C and assembly (built by `aws-lc-sys`) | `CC`/`CXX` set to the toolchain's `<triple>-<san>-clang` wrappers |
| libc++ and the toolchain's C components | the add-on's instrumented sysroot |
| C ABI consumers (`tests/abi.c`, `tests/static.c`) | the same wrappers, linked with `-shared-libsan` |

Each run has three stages, selectable with `--stages`:

1. **rust**: the full unit and integration suite under Nextest.
2. **native**: a sanitized `libbemo_ffi` static archive linked into the static C
   consumer.
3. **jvm** (Linux only): a stock JVM with the shared sanitizer runtime in
   `LD_PRELOAD` runs the FFM, C ABI, and every Netty transport and TLS contract
   against the sanitized shared library.

Instrumented builds select `rust-allocator` with default features disabled, so
native buffer allocations go through the sanitizer's allocator. Ordinary tests
still exercise bundled mimalloc on non-Apple hosts; sanitizer success does not
qualify mimalloc itself. Rust links with `-nodefaultlibs`, so the toolchain's
compiler-rt builtins archive is passed explicitly (TSAN cleanups need
`__gcc_personality_v0` from it).

### The JVM stage

HotSpot is not instrumented, so the JVM stage is configured to avoid both false
reports and blind spots:

- ASAN runs with `handle_segv=0:allow_user_segv_handler=1`, because HotSpot uses
  SIGSEGV for safepoints and implicit null checks, and with `detect_leaks=0`,
  because the JVM never frees much of its own metadata. Leak checking still runs
  in the rust and native stages.
- TSAN runs with `ignore_noninstrumented_modules=1`, so interceptor calls made
  directly by libjvm and the JDK's native libraries (their mutexes and `malloc`)
  are not reported. This can only remove synchronization that TSAN could see; it
  never hides accesses made by instrumented Rust or C code, including those
  reached through an FFM downcall. Do not add `race:` suppressions that match
  Bemo frames.

Before any contract runs, `tests/sanitizer` provides a **canary**: a small C
library called from Java through FFM that overflows a heap allocation (ASAN) or
races on a counter from four JVM threads (TSAN). The stage fails unless the
sanitizer reports the canary under the exact launcher, options, and runtime used
for the contracts. A clean contract run therefore counts as evidence.

GraalVM Native Image executables cannot be sanitized: they are statically
linked, uninstrumented images with their own heap and signal handling. The
same Rust code is covered through the C and JVM stages instead.

### Evidence

Evidence lives under `build/reports/verification/<mode>/`: command logs,
JUnit reports for the JVM stage (`jvm/`), the canary log, and `manifest.json`
recording the compiler, target, toolchain version, whether the add-on was
present, the instrumented components, the stages that ran, and the result.
Failures retain their evidence and fail CI.

## Miri

Miri interprets the core unit tests and the buffer, ABI, pool, and workload
contracts. Test-specific exclusions cover real socket drivers and AWS-LC calls.
Filesystem isolation is disabled to permit the workload header contract;
borrow, initialization, and race checks remain enabled. Reports show each
interpreted and ignored test.

## Fuzzing

The separate `fuzz` workspace pins libFuzzer and its dependency lockfile. Each
target asserts invariants, not only the absence of crashes:

| Target | Property |
| --- | --- |
| `http` | HTTP/1 request parsing gives identical heads, bodies, framing, and errors however the input is fragmented; every receive allocation is released |
| `buffers` | Retained and split views preserve bytes and release capacity exactly once |
| `abi` | Generated handle sequences, including stale releases and invalid admissions, keep owner accounting exact |
| `response` | Encoded response heads parse with a reference parser (`httparse`) into exactly the caller's non-owned headers, in order, with one consistent framing header; invalid statuses and headers are rejected before allocation |
| `gzip` | Each gzip member decodes with a reference decoder (`flate2`) to its input, encoders are reusable, and output storage returns to its budget |
| `tls` | A Bemo client and server exchange data over a wire the fuzzer fragments, corrupts, truncates, duplicates, or drops: plaintext is always a prefix of what the peer sent, an untampered exchange always completes, errors are terminal, and all TLS storage returns to its budget |
| `h2` | Arbitrary client frames after the preface reach the native HTTP/2 server, driven like an application that responds, streams, acknowledges, and releases: stream events follow their lifecycle, over-acknowledgement is refused, frame marks never describe unproduced bytes, and the stream limit holds |

`h2` reaches crate-private state through `bemo::fuzzing`, compiled only with the
non-default `fuzzing` feature; it is not API. Inputs are bounded to 32 KiB.

Checked-in seeds live in `fuzz/seeds/<target>/` and minimized findings in
`fuzz/regressions/<target>/`. Both replay as ordinary tests in `make test`
(`cargo test --manifest-path fuzz/Cargo.toml --lib`), so a finding stays fixed
on every platform without a fuzzing toolchain, and both seed each fuzzing run.

`make fuzz` runs each target for 30 seconds and `make fuzz-smoke` for 1,000
executions; extended runs use `python3 tools/verify.py fuzz --seconds 600`.
Fuzzing uses cargo-fuzz's ASAN build, which instruments Rust but not AWS-LC.
Generated corpora and crash artifacts are ignored by Git and archived in CI.

To handle a finding:

1. Reproduce it with `cargo fuzz run --target <host-triple> <target>
   fuzz/artifacts/<target>/<artifact>`.
2. Minimize it with `cargo fuzz tmin`.
3. Decide whether the code or the harness is wrong. A harness must model the
   real caller's contract: for example, the TLS harness keeps acknowledging
   transmission until a step reports `NeedTransmit`, exactly like the native
   HTTP lane.
4. Commit the minimized input under `fuzz/regressions/<target>/` with a
   descriptive name, together with the fix.

The safety workflow joins the Ready gate alongside ordinary correctness and
benchmark checks.
