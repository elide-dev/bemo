# PR integration and dependency update plan

**Goal:** Integrate all 23 open Bemo PRs and update dependencies, prioritizing Myna and all binding seams.

**Architecture:** Preserve the Rust core boundary, paired FFM/C API contracts, and pinned external Cargo forks. Use Cargo for Rust and Elide for Java compilation and resolution.

- [x] Create an integration branch from current main and cherry-pick all active PR commits with provenance; retain every regression test when resolving conflicts.
- [x] Update Myna to v0.6.0, Elide to 1.6.1, and GraalVM SDK/toolchain to 25.5.5-elide.21; refresh compatible dependencies and lockfiles.
- [x] Regenerate seams, inspect ABI changes, and validate both bindings and the ThinLTO consumer.
- [x] Run make fmt, make build, make check, make test, make test-native-image, and tools/test_git_dependency.py; resolve integration failures.

Existing untracked launch briefing is outside this work. Do not publish or merge upstream PRs.
