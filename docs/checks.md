# Development tools and strict checks

Install Mise, then run `mise install` and `mise exec -- make check`. The checked-in
Mise lock records tool download checksums. `rust-toolchain.toml` separately pins
Rust nightly and its Clippy/rustfmt components. Elide continues to own dependency
resolution, Java compilation, formatting, and JAR generation; Cargo owns Rust.
The stock JVM CI matrix sets `JAVA_HOME` after Mise setup so Java 22 compatibility
is tested independently of the development JDK and Elide's compiler JVM.

Mise installs hk hooks with `hk install --mise`. `hk check --all` runs the same
`make check` gate as CI; `hk fix --all` runs `make fmt`. Pre-commit checks use a
stash to check the staged contents without rewriting partially staged changes.
A single hook step serializes checks that share build output directories.

`make check` verifies generated exports, Cargo and Java formatting, strict
workspace Clippy, Rust documentation, version consistency, cargo-deny, module
compilation, and Java analyzers. `make test` runs Nextest, doctests, the external
Cargo Git consumer, and the JVM/C ABI contracts. `make test-native-image` checks
both statically linked metadata and Netty/TLS contracts; it requires GraalVM's
`native-image` in `JAVA_HOME` or on `PATH`.

## Java analysis

`tools/check_java.py` launches javac through Elide with isolated analysis outputs:

- Error Prone's enabled checks and javac warnings are fatal across all production
  modules. Narrow suppressions document Netty identity tokens, the required
  generic registration signature, and backing-array pinning with offsets applied
  separately. There are no global Error Prone check exclusions.
- NullAway checks JSpecify null-marked production packages in JSpecify mode.
  Optional state and nullable boundary arguments are explicit. The Netty-package
  ALPN bridge is marked at class level so Dokar does not impose package defaults
  on Netty's own classes. Lifecycle accessors require registration/initialization
  before returning non-null state.
- Checker Framework Regex and Formatter checkers cover API and Netty sources.
  CF 4.3.0 crashes on signature-polymorphic `MethodHandle.invokeExact` in the FFM
  adapter (`DefaultTypeHierarchy`, primitive versus array). The low-level FFM
  and Native Image adapters therefore use Error Prone/NullAway; they are not
  represented as Checker Framework-verified. CF's unclaimed-annotation warning
  is disabled because these processors intentionally ignore unrelated JFR,
  JSpecify, and native annotations; other warnings remain fatal.

Negative fixtures must fail with the expected NullAway, Error Prone, Regex, and
Formatter diagnostics. Logs live under `build/checks/fixtures/`; an unrelated
compiler crash does not satisfy the fixture check. JSpecify is an API compile
dependency in Maven. Analyzer processors and Checker Framework remain build-only;
no GraalVM or Elide runtime dependency is added to API/FFM artifacts.

## Rust policy

Both crates inherit the workspace policy, including unsafe-operation checks,
unsafe-block explanations, pointer-cast checks, debug/TODO bans, and selected
allocation/copy checks. FFI documentation remains mandatory. Release overflow
checks are enabled. `frame_chunk` requires an unsafe call because initialized
payload bytes may come from foreign writes; callers must establish that invariant.

The only `mem::forget` exceptions retain buffers that may still be kernel-owned
after failed polling or incomplete retirement. Each use carries its own lint
expectation and reason; cancellation alone never authorizes freeing storage.

`cargo-deny --locked --workspace check` audits all features and the Linux,
macOS, and Windows dependency graphs. It rejects vulnerabilities, yanked crates,
multiple versions, wildcard requirements, unapproved licenses, and unknown Git
or registry sources. The three approved Git forks must specify revisions. The
current graph needs no advisory or duplicate-version exceptions.
