# svmgen and bitcode implementation plan

> **For agentic workers:** Use superpowers:executing-plans to implement task by task. The user explicitly waived further spec/plan review and asked for autonomous execution.

**Goal:** Generate Native Image imports and stage a verified ThinLTO archive variant.

**Architecture:** A pinned generator produces checked-in Java imports and per-target seam metadata. Existing adapters preserve handles, callbacks, and ownership. A separate Cargo output directory supplies a bitcode staticlib and provenance for downstream ThinLTO.

**Tech Stack:** Cargo, Rust LLVM 23, Elide Java tools, svmgen, Clang/LLVM.

**Spec:** ../specs/2026-10-06-svmgen-bitcode-design.md

## Global Constraints

All boundaries and verification requirements in the spec apply. The generator checkout must be refreshed between phases. Publishing is separate from staging.

## Review Focus

- Descriptor signature/layout drift must fail before Java compilation.
- Pointer adaptation must preserve zero addresses and valid output storage.
- Callback imports retain their function-pointer and isolate context types.
- Native-only dependency members must be reported accurately.
- Staged classifier metadata must describe the actual archive and toolchain.

## Task 1: Generated imports

Files: `seams/bemo.seam`, `tools/seam.py`, `tools/test_seam.py`, `tools/versions.json`, native-image Java adapters and generated imports, `tools/build.py`.

Interfaces: `seam.generate(check=False, target=None)` validates header signatures and writes/checks Java; `seam.target_triple()` identifies the supported host target; `seam.output(target)` supplies target metadata.

- [x] Write negative tests for signature drift, callback inventory, and unexpected generated Java shape; run `python3 tools/test_seam.py` and observe failure.
- [x] Implement pinned checkout compilation using Elide, descriptor verification, generation, formatting, and C context/library integration.
- [x] Replace supported native declarations with typed delegation, retaining callback declarations.
- [x] Run unit tests and `make build`, `make check`, `make test-native-image`.

## Task 2: Bitcode archives and consumer evidence

Files: `tools/bitcode.py`, `tools/test_bitcode.py`, `tools/build.py`, `Makefile`, `tools/verify_package.py`.

Interfaces: `bitcode.build()` returns the staged staticlib path; `bitcode.verify(archive, directory)` inventories members and links/runs a real consumer.

- [x] Write tests rejecting native-only archives and malformed member data; run tests and observe failure.
- [x] Build only bemo-ffi staticlib via Cargo linker-plugin LTO in an isolated target directory; inspect member magic and record flags/compiler/generator provenance.
- [x] Stage a `-thinlto` Native Image classifier with headers and seam metadata; verify the extracted archive through Clang ThinLTO and inspect saved optimized IR.
- [x] Run `make build-bitcode`, `make package`, and `python3 tools/verify_package.py`.

## Task 3: CI, publishing inventory, and verification

Files: build workflow, release/package tooling and tests, `docs/native-loading.md`, `docs/publishing.md`, integration documentation.

- [x] Refresh svmgen and incorporate compatible upstream changes.
- [x] Include the new classifier in local staging/release inventories without publishing.
- [x] Configure compatible LLVM tools for package verification on CI.
- [x] Run `make fmt`, `make build`, `make check`, `make test`, `make test-native-image`, external Cargo dependency contract, Python contracts, and local package verification.
- [x] Review final diff and record actual platform/toolchain limitations and verification results.

## Decisions

The refreshed upstream revision supports Java package/class naming; use it directly. A tiny deterministic adaptation adds only C context/library annotations. Callback generation stays handwritten until upstream models it. Separate bitcode archives are the selected deliverable; no fat-container claim is necessary.

## Completion evidence

Implemented with svmgen 0.2.0 revision `1bde5a11b1f25446e6410c27e6484546b4379c7d` after repeated upstream refreshes. Astra reviewed the initial design and final implementation within the requested three-minute review budgets. Incorporated portable Java generation, tracked-source compilation, correct nullable probes/empty buffers, and pinned checksummed LLVM provisioning.

Passed `make fmt`, `make build`, `make check`, `make test`, `make test-native-image`, `make package`, extracted-package verification, 47 Python contracts, and cross-target generation freshness. `make test` includes the external Cargo dependency contract. JVM tests and package verification used `BEMO_TEST_JAVA` to select a working stock JDK because the local Elide JDK subprocess helper failed to launch OpenSSL. Native Image ABI/callback/TLS contracts ran against the new typed carriers.

The staged macOS ARM64 archive contains 307 bitcode and 524 native members. The extracted classifier linked and ran its ownership consumer through ThinLTO, and saved optimized linker IR contained a Bemo definition. Linux CI is configured but was not executed locally. LLVM provisioning extraction/checksum contracts passed; the large upstream distribution downloads were not installed locally, where LLVM 23.1.1 was already available.

Ruling: select the AWS-LC cc builder and LLVM AR/RANLIB on macOS. A direct reproduction showed Apple ar omitting bcm bitcode as an unknown architecture, causing a missing CPU setup symbol. The LLVM-archived dependency passed the actual consumer link.
