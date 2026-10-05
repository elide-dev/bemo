# Active work plan

Keep this checklist current as requests arrive. A configuration edit is not
complete until its checks have been exercised; record limitations explicitly.

## Before the next push

- [x] Investigate the Windows failure in
  `bemo::http native_h2_multiplexes_responses_over_verified_tls` using CI evidence.
- [x] Fix the cause or establish a supported diagnosis, and run the relevant
  regression checks. Do not mask the failure with retries.
- [x] Implement and locally verify the fix before pushing the pending commits.

CI recorded `ConnectionAborted` when polling H2 connection completion. The
platform-specific reciprocal TLS shutdown is the suspected cause. The test adapter
now accepts platform disconnect errors only after authenticated TLS close_notify;
application reads/writes and unauthenticated EOF remain errors. Its regression
contract and the H2 exchange passed 20 consecutive local runs without retries.
Windows Rust verification passed in CI run `37228353422`, including the H2/TLS
regression. This local host is macOS.

## JVM strictness

- [x] Inspect the existing Elide compilation and dependency setup.
- [x] Resolve pinned Error Prone, NullAway, JSpecify, and Checker Framework dependencies.
- [x] Integrate Error Prone and NullAway into the Elide-owned compilation checks.
- [x] Apply JSpecify package defaults and correct nullable API contracts.
- [x] Integrate Checker Framework checks with an explicit, documented scope.
- [x] Verify analyzers reject representative invalid code.
- [x] Preserve published dependency isolation and validate JVM contracts.

## Rust strictness

- [x] Compare Bali's lint and dependency policies with Bemo's workspace.
- [x] Enable inherited workspace lints for the core and resolve findings.
- [x] Preserve existing documentation checks on the FFI crate.
- [x] Add cargo-deny policy with reviewed dependency exceptions where needed.
- [x] Validate Clippy, formatting, documentation, and the Git dependency consumer.

## Toolchains, formatting, and hooks

- [x] Pin development tools with Mise and check in the tool lockfile.
- [x] Configure hk hooks around the same checks used in CI.
- [x] Wire Cargo fmt and Elide-driven Java formatting into hooks and CI.
- [x] Document setup and the supported check commands.

## Continuous benchmarks

- [x] Use `linux-amd64-bench` for wall-time benchmarks.
- [x] Move benchmark jobs into the reusable verification flow.
- [x] Update baseline artifact lookup and workflow permissions.
- [x] Preserve trusted-runner restrictions and review scheduled execution.
- [x] Update measurement documentation and validate workflow wiring.

Benchmark workflow wiring passes Actionlint; execution will be verified in CI.

## Release automation and supply-chain integrity

- [x] Configure Release Please for coordinated Cargo and Maven version updates.
- [x] Implement staging and verification before immutable release publication.
- [x] Configure SLSA Level 2 provenance and document the build trust boundary.
- [x] Configure Sigstore artifact signing and document consumer verification.
- [ ] Qualify provenance, signatures, and immutability with the first real CI release.
- [x] Verify repository settings, workflow permissions, and release-only credentials.

## Verification and delivery

- [x] Run the relevant `make build`, `make check`, `make test`, and
  `make test-native-image` checks, recording unavailable checks explicitly.
- [x] Review the complete diff for consistency across local commands and CI.
- [x] Commit coherent changes, with verification results recorded.
- [x] Push authorized changes and report CI results or pending checks accurately.

## Current verification notes

- Analyzer dependencies resolved successfully through `make deps`.
- `make check` passed, including four analyzer rejection contracts, Clippy,
  documentation, formatting, and cargo-deny without dependency exceptions.
- `make test` passed: 299 Rust tests, doctests, the external Git dependency
  contract, and the JVM/C ABI contracts.
- `make test-native-image` passed both metadata and full Netty/TLS contracts.
  Verification caught and corrected an unsynchronized handshake-promise initializer
  in the pending Java strictness changes.
- `make build`, `make package`, and `tools/verify_package.py` passed, including
  Maven metadata, resource extraction, FFM TLS, and static linkage.
- Release automation passes Actionlint and six staging/publication contract tests. Build
  provenance and signing require a real CI release to verify the deployed flow.
- Repository release immutability is enabled; the release environment only
  permits deployments from `main`. No Central publishing credentials are needed.

## CI follow-up: run 37228353422

- [x] Trace the Ready failure to Linux Clippy and wall-time runner setup.
- [x] Document unsafe test invariants in Linux-only seccomp, ABI, and serving paths.
- [x] Replace untyped affinity-mask zeroing with an initialized word array.
- [x] Bootstrap rustup and export Cargo's bin directory in shared setup, following Bali.
- [x] Confirm Linux lint and JVM checks in CI run `37234380668`.
- [x] Resolve wall-time benchmark manifest drift; confirmed in run `37239018787`.

Windows, macOS, Native Image, packaging, coverage, simulation benchmarks, and
transport measurements passed in this run. Linux JVM jobs stopped at Rust
Clippy before reaching Java checks. The wall-time runner stopped at setup before
compiling benchmarks because Cargo was absent from PATH.

## H2 shutdown follow-up: run 37234380668

- [x] Investigate Linux H2 upload/expectation test failure (`BrokenPipe`).
- [x] Correct the test adapter's assumption that H2 completion implies TLS EOF
  has already been authenticated: GOAWAY can finish H2 first.
- [x] After a reciprocal shutdown write fails, process pending TLS input before
  accepting the disconnect; retain failures for unauthenticated EOF or unread data.
- [x] Verify a deterministic fault-injection test fails before the fix and passes
  after it for TLS 1.2/1.3 and BrokenPipe/ConnectionAborted/ConnectionReset.
- [x] Run the Rust suite and 20 focused H2 runs without retries.
- [x] Confirm the fix on Linux CI (`37235296694`); Windows and macOS also passed.

The same run completed the TLS Criterion benchmarks, then failed the end-to-end
wall-time manifest check (`Benchmark inputs or artifacts changed`). That separate
benchmark issue remains open; no integrity check has been disabled.

## Benchmark manifest follow-up: run 37235296694

- [x] Reproduce source fingerprint drift with Cargo environment unavailable:
  Criterion writes reports under `crates/bemo/target/criterion`.
- [x] Set an explicit CI Criterion report directory and exclude generated target
  directories from source hashing; retain class and native-library checks.
- [x] Upload Criterion evidence and write actionable manifest-drift diagnostics.
- [x] Verify generated reports leave the manifest unchanged while source edits,
  source deletion, class changes, and native-library changes invalidate it.
- [x] Run all eight transport benchmark cases as smoke tests, Python contracts,
  the external Cargo consumer, and Actionlint.
- [x] Confirm the wall-time job succeeds on the dedicated CI runner (`37239018787`).

Every other verification job passed in run `37235296694`.

## Release token follow-up: run 37239018787

- [x] Confirm repository immutability remains enabled with authenticated admin access.
- [x] Remove settings-endpoint calls requiring Administration permission, which
  the workflow GITHUB_TOKEN cannot hold, from both workflow and publisher.
- [x] Preserve the published release check: `immutable` must explicitly be true.
- [x] Test successful publication, false/missing immutable status, and rejection
  before publication of incomplete signatures or mismatched uploaded digests.
- [x] Document that repository administrators own the policy and CI's check is
  post-publication; no administrator credential is added to workflows.
- [ ] Confirm the corrected release job in CI.


## Standalone qualification audit

- [x] Canonicalize the renamed Cargo.lock and prepare the wall-time JVM manifest
  after the CodSpeed Rust build; retain strict lock/source/artifact fingerprints.
- [x] Add stock Netty epoll/kqueue and NIO comparison paths, identical JDK TLS,
  explicit native TLS measurements, latency and CPU metrics, and concurrency selection.
- [x] Run the eight workloads against stock OpenJDK and macOS kqueue; retain raw samples.
- [x] Add bounded parser, buffer, and ABI fuzz targets with pinned dependencies and seeds.
- [x] Add ASAN, TSAN, and scoped Miri commands and required CI jobs; verify locally.
- [x] Add native compatibility requirements, package checks, and CI evidence artifacts.
- [x] Update repository URLs, qualification docs, and low-level API method descriptions.
- [x] Supply dependency-owned C headers through Cargo metadata and verify an external consumer.
- [ ] Verify the new Linux safety and comparison jobs in CI.
- [ ] Reconnect or confirm the renamed repository's CodSpeed registration.
- [ ] Complete and verify Elide's pinned-source cutover in its separate worktree.
- [ ] Qualify a first signed release after the full merged-source gate succeeds.

Local macOS verification passed the ordinary build/check/test/Native Image and
packaged-library contracts. ASAN and TSAN each passed 300 Rust tests. Scoped Miri
passed with its explicit driver/crypto exclusions; each fuzzer passed 1,000
executions. Sanitizer scope and its C/assembly/JVM exclusions are documented in
`native-safety.md`. Benchmark comparisons are workload-dependent; no broad claim
of outperforming Netty's native transports is supported.
