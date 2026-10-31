# October 2026 integration and dependency refresh

The integration branch starts at `26fb123` and cherry-picks all 23 PRs open on
2026-10-31 (27 commits), retaining original commit provenance:
#51, #52, #53, #54, #62, #63, #64, #65, #66, #67, #68, #70, #71,
#73, #74, #75, #76, #77, #83, #84, #85, #86, and #87.

Conflicts combined independent HTTP/2 and TLS regression tests, retained both
Rustls and AWS-LC updates, and registered every new JVM contract alongside the
framework adapter contracts. No upstream PR was merged or closed.

## Dependency changes

| Component | Previous | Updated |
| --- | --- | --- |
| Elide | 1.5.4+20260921 | 1.6.1 |
| Myna revision | 34a0213 | 66ee758 (v0.6.0) |
| GraalVM SDK and Native Image | SDK 25.3.4.1, external compiler | 25.5.5-elide.21, checksummed compiler |
| Netty | 4.2.18.Final | 4.2.19.Final |
| Ktor | 3.6.0 | 3.6.1 |
| Kotlin | 2.4.20 | 2.4.21 |
| Spring Framework | 7.0.9 | 7.0.10 |
| Spring Boot | 4.1.1 | 4.1.2 |
| Reactor | 3.8.7 | 3.8.8 |
| Error Prone | 2.50.0 | 2.51.0 |
| NullAway | 0.14.2 | 0.14.3 |
| Rustls | 0.23.45 | 0.23.46 |
| AWS-LC Rust | 1.18.1 | 1.18.2 |
| Tokio | 1.53.2 | 1.54.1 |
| crc32fast | 1.5.2 | 1.5.3 |
| libc | 0.2.190 | 0.2.191 |
| Pillow | 12.3.0 | 13.0.0 |

Cargo lockfiles were refreshed for the root, fuzz, embedder, and compression
workspaces. Existing revision-pinned forks remain pinned in dependency
declarations. The fuzz Rustls oracle and compression CRC pin were synchronized.
Mise's Elide archive checksums were regenerated. Framework example manifests
use the same SDK repository and matching versions.

Myna regeneration changes the canonical fingerprint without changing public
signatures. Native Image tests and native example builds select the matching
GraalVM compiler through `tools/setup_graalvm.py`. Linux x86_64 and macOS ARM64
archives are checksum-pinned in `tools/versions.json`. The generic archive
extractor now accepts both gzip and XZ, with an extraction regression test.

Error Prone's new arena diagnostic is suppressed only at the existing FFM
constructor ownership transfer: construction failure closes the arena, and
successful construction transfers it to the binding's `close()` method.

## Validation

Passed on macOS ARM64:

- `make build`, `make check`, and `make fmt` (including format verification in `make check`).
- `make test`: 386 Rust tests, seven fuzz-corpus replay tests, and 28 JVM/FFM contracts.
- `make test-native-image`: all three executables passed using the checksummed `.21` toolchain.
- `python3 tools/test_git_dependency.py`: external revision-pinned Cargo consumer passed.
- Myna generation checks for macOS ARM64 and Linux x86_64; unchanged public signature inventory.
- `make test-bitcode`: contracts applied to 97 functions; 309 bitcode members and 524 native members retained; linked ownership consumer passed and Bemo definitions appeared in optimized IR.

The ThinLTO run selected the installed macOS 15.4 SDK through `SDKROOT`.
The default Xcode SDK contains `arm64e.x1` TAPI targets unsupported by the
existing LLVM 23.1.2 linker. Linux execution remains CI's qualification scope.

The existing `hk.pkl` hook fails during configuration evaluation with
`union property 'command' has no selected default`. Cherry-pick continuation
and the local refresh commit used `HK=0`; the prescribed checks ran directly.

