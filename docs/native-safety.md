# Native safety verification

The ordinary contract suite exercises the production allocator, native drivers,
TLS, HTTP, ownership, and both JVM bindings. Additional verification is available
through the same pinned Rust toolchain:

```sh
rustup component add rust-src miri
cargo install cargo-fuzz --version 0.13.1 --locked
make test-asan
make test-tsan
make test-miri
make fuzz-smoke
make fuzz
```

ASAN and TSAN rebuild Rust's standard library and run the complete Rust unit and
integration suite with explicit host targeting. They select `rust-allocator` with
default features disabled, making native buffer allocations visible to the
instrumented allocator. Ordinary tests continue to exercise bundled mimalloc on
non-Apple hosts; sanitizer success does not qualify mimalloc's implementation.
ASAN enables leak detection on Linux. It is disabled on macOS.

The runs instrument Rust and Rust's standard library. AWS-LC C/assembly, the JVM,
Netty's native dependencies, and GraalVM Native Image are outside this
instrumentation scope. Successful JSSE/OpenSSL contracts and Native Image tests
are separate evidence, not sanitizer coverage of those components.

Miri interprets the core unit tests and the buffer, ABI, pool, and workload
contracts. Existing test-specific exclusions cover real socket drivers and
AWS-LC calls. Filesystem isolation is disabled to permit the workload header
contract; borrow, initialization, and race checks remain enabled. Reports show
each interpreted and ignored test.

The separate `fuzz` workspace pins libFuzzer and its dependency lockfile. Its
three targets check HTTP fragmentation invariance, retained-buffer contents and
budget accounting, and generated ABI handle lifetimes including stale releases.
Inputs are bounded to 32 KiB; ABI sequences use at most 256 instructions.
Checked-in seeds cover fixed-length and chunked bodies, buffer retention, and
handle operations. Fuzzing runs with ASAN and the instrumented standard library.

`make fuzz` runs each target for 30 seconds; `make fuzz-smoke` runs each for 1,000
executions. Extended runs use `python3 tools/verify.py fuzz --seconds 600`.
Generated corpus additions and crash artifacts are ignored by Git and archived
in CI. Reproduce a finding with `cargo fuzz run --target <host-triple> <target>
fuzz/artifacts/<target>/<artifact>`; minimize it with `cargo fuzz tmin` before
adding the regression to the normal suite.

Evidence lives under `build/reports/verification/<mode>/`: command logs and a
manifest with the compiler, target, allocator, instrumentation scope, and result.
Failures retain their evidence and fail CI. The safety workflow joins the Ready
gate alongside ordinary correctness and benchmark checks.
