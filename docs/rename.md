# Bemo rename and release plan

Rename the unreleased project before publishing `0.2.0`. The release PR is held
as a draft while the rename is validated. Keep the current snapshot and Cargo
versions; Release Please will update the release branch after this change merges.

| Surface | New name |
| --- | --- |
| Project and Elide build | Bemo / `bemo` |
| Rust workspace crates | `bemo`, `bemo-ffi` |
| Java packages and Maven group | `dev.elide.bemo` |
| Maven artifacts | `bemo-api`, `bemo-ffm`, `bemo-native-image`, `bemo-netty` |
| Native library stem | `bemo_ffi` |
| Metadata header and symbols | `bemo.h`, `bemo_abi_version`, `bemo_capabilities` |
| Loader properties | `bemo.native.path`, `bemo.native.workdir` |
| Stock JVM test override | `BEMO_TEST_JAVA` |
| Platform release bundle | `bemo-VERSION-PLATFORM-unsigned.zip` |

Apply the rename to source directories, imports, bindings, generated exports,
native resource metadata, Cargo manifests and lockfile, packaging, benchmarks,
coverage path mappings, Release Please updates, and documentation together.
Regenerate the C forwarding boundary and run the standard formatters.

The existing `elide_transport_*` ABI 3 symbol names, shared transport header,
layouts, capabilities, ownership rules, and allocator behavior stay unchanged.
Both Java bindings use the renamed metadata symbols and library. No compatibility
aliases are needed because no release has been published.

The GitHub repository is now `elide-dev/bemo`. Repository URLs and signing
identities use that name. Codecov/CodSpeed registrations must follow the hosting
rename before publishing new performance or provenance evidence. Cargo.lock
must keep renamed packages in Cargo's canonical order; otherwise its first
normalization changes the benchmark source fingerprint.

Validate with `make build`, `make check`, `make test`, `make test-native-image`,
the Python tool tests including `tools/test_git_dependency.py`, `make package`,
and `tools/verify_package.py`. Inspect packaged namespaces, classifier resources,
metadata exports, and preserved transport exports. Keep API/FFM dependencies free
of GraalVM and Elide runtime types. Stage artifacts locally; publishing remains
a separate release step after the renamed release PR is verified.

## Implementation validation

The renamed snapshot passed all commands above on macOS ARM64, including FFM,
Native Image, automatic loading from the actual classifier JAR, and static C
linkage against the packaged headers and archive. The Python tool suite passed
all 13 tests, including a fresh external Git consumer of `bemo` and `bemo-ffi`.
The built shared library exposes both Bemo metadata symbols and exactly the 85
transport exports declared in the unchanged shared transport header.

The package verifier additionally checks that published Java classes use
`dev/elide/bemo/` (apart from Netty's ALPN bridge), that native-image metadata uses
the Bemo artifact path, and that the shared-library classifier contains the Bemo
library name. Cross-platform CI remains part of release qualification.
