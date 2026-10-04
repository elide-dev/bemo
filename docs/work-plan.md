# Active work plan

Keep this checklist current as requests arrive. A configuration edit is not
complete until its checks have been exercised; record limitations explicitly.

## JVM strictness

- [x] Inspect the existing Elide compilation and dependency setup.
- [x] Resolve pinned Error Prone, NullAway, JSpecify, and Checker Framework dependencies.
- [ ] Integrate Error Prone and NullAway into the Elide-owned compilation checks.
- [ ] Apply JSpecify package defaults and correct nullable API contracts.
- [ ] Integrate Checker Framework checks with an explicit, documented scope.
- [ ] Verify analyzers reject representative invalid code.
- [ ] Preserve published dependency isolation and validate JVM contracts.

## Rust strictness

- [x] Compare Bali's lint and dependency policies with Dokar's workspace.
- [ ] Enable inherited workspace lints for the core and resolve findings.
- [ ] Preserve existing documentation checks on the FFI crate.
- [ ] Add cargo-deny policy with reviewed dependency exceptions where needed.
- [ ] Validate Clippy, formatting, documentation, and the Git dependency consumer.

## Toolchains, formatting, and hooks

- [ ] Pin development tools with Mise and check in the tool lockfile.
- [ ] Configure hk hooks around the same checks used in CI.
- [ ] Wire Cargo fmt and Elide-driven Java formatting into hooks and CI.
- [ ] Document setup and the supported check commands.

## Continuous benchmarks

- [x] Use `linux-amd64-bench` for wall-time benchmarks.
- [x] Move benchmark jobs into the reusable verification flow.
- [x] Update baseline artifact lookup and workflow permissions.
- [x] Preserve trusted-runner restrictions and review scheduled execution.
- [x] Update measurement documentation and validate workflow wiring.

Benchmark workflow wiring passes Actionlint; execution will be verified in CI.

## Release automation and supply-chain integrity

- [x] Configure Release Please for coordinated Cargo and Maven version updates.
- [ ] Stage and verify all release artifacts before publishing an immutable release.
- [ ] Configure SLSA Level 2 provenance and document the build trust boundary.
- [ ] Sign release artifacts with Sigstore and document consumer verification.
- [x] Verify repository settings, workflow permissions, and release-only credentials.

## Verification and delivery

- [ ] Run the relevant `make build`, `make check`, `make test`, and
  `make test-native-image` checks, recording unavailable checks explicitly.
- [ ] Review the complete diff for consistency across local commands and CI.
- [ ] Commit coherent changes, with verification results recorded.
- [ ] Push authorized changes and report CI results or pending checks accurately.

## Current verification notes

- Analyzer dependencies resolved successfully through `make deps`.
- The initial stricter Clippy run failed: 188 missing unsafe-block safety comments,
  six pointer-alignment findings, three `mem::forget` findings, and two large
  stack-array findings. Review the ownership and alignment invariants before
  fixing these; do not suppress them wholesale.
- Release automation passes Actionlint and four rejection-path tests. Build
  provenance and signing require a real CI release to verify the deployed flow.
- Repository release immutability is enabled; the release environment only
  permits deployments from `main`. No Central publishing credentials are needed.
