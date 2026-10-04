# Continuous integration

The layout follows Elide's `on.*`, `check.*`, and `job.*` reusable workflows.
PR, main-branch push, merge queue, and manual verification use the same graph.
Every change runs the graph; no path-based skips can silently omit a cross-ABI
check. `Ready` fails unless every required job succeeds, including failed or
cancelled dependencies. Configure branch protection to require the resulting
`verify / Ready` status after the first GitHub run confirms its displayed name.

| Job | Coverage |
| --- | --- |
| Check Rust | Linux/macOS/Windows; format, Clippy, tests, doctests, release build, external allocator check; forced polling and external Git consumer on Linux |
| Check JVM | Linux/macOS; Temurin 22/25; format, Java warning checks, Rust docs, C ABI and complete Netty FFM contracts |
| Check Native Image | Linux; GraalVM 25; compile, statically link, and execute complete Netty C API contracts |
| Build | Linux/macOS; release Cargo libraries, JVM/source/Javadoc JARs, POMs, checksums, native classifiers; packaged Netty TCP/TLS and FFM contracts |

Actions are SHA-pinned following Elide's existing pins. Linux jobs use runner
hardening in audit mode, checkout does not persist credentials, and the default
token is read-only. Fork PRs receive no publishing secrets. Jobs have bounded
timeouts and matrices keep running to expose independent platform failures.
Dependabot tracks Cargo dependencies and GitHub Actions; JVM/toolchain pins live
in `tools/versions.json`, `.elide-version`, and `rust-toolchain.toml`.

Hosted runners are the default; this repository does not require Elide's private
CI pool. Python 3.11+, Rustup, and the platform C compiler are expected from the
specified hosted images. Native Image static builds need a compatible GraalVM
JDK and development headers. The build setup installs the selected JDK and
pinned Elide, and Rustup reads this checkout's pinned toolchain.

`Stage Release` runs the verification graph and uploads unsigned repositories.
It does not publish to Central or create a GitHub release. See publishing.md for
the remaining release steps. The repository must be hosted and workflows run
before remote CI success can be asserted.
