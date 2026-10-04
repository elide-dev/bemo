# Active work plan

Keep this checklist current as requests arrive. A configuration edit is not
complete until its checks have been exercised; record limitations explicitly.

## Before the next push

- [x] Investigate the Windows failure in
  `dokar::http native_h2_multiplexes_responses_over_verified_tls` using CI evidence.
- [x] Fix the cause or establish a supported diagnosis, and run the relevant
  regression checks. Do not mask the failure with retries.
- [x] Implement and locally verify the fix before pushing the pending commits.

CI recorded `ConnectionAborted` when polling H2 connection completion. The
platform-specific reciprocal TLS shutdown is the suspected cause. The test adapter
now accepts platform disconnect errors only after authenticated TLS close_notify;
application reads/writes and unauthenticated EOF remain errors. Its regression
contract and the H2 exchange passed 20 consecutive local runs without retries.
Windows execution remains pending CI; this host is macOS.

## JVM strictness

- [x] Inspect the existing Elide compilation and dependency setup.
- [x] Resolve pinned Error Prone, NullAway, JSpecify, and Checker Framework dependencies.
- [x] Integrate Error Prone and NullAway into the Elide-owned compilation checks.
- [x] Apply JSpecify package defaults and correct nullable API contracts.
- [x] Integrate Checker Framework checks with an explicit, documented scope.
- [x] Verify analyzers reject representative invalid code.
- [x] Preserve published dependency isolation and validate JVM contracts.

## Rust strictness

- [x] Compare Bali's lint and dependency policies with Dokar's workspace.
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
- [ ] Push authorized changes and report CI results or pending checks accurately.

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
