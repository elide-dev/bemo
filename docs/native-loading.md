# Native loading and Java namespaces

All Maven artifacts use group `dev.elide.bemo`. Java transport APIs now live in
`dev.elide.bemo.transport`, TLS adapters in `.transport.tls`, and their Native
Image binding in `.transport.svm`. The smaller metadata ABI remains in
`dev.elide.bemo`, with its FFM and C API bindings in `.ffm` and `.svm`. The only
`io.netty` class is the ALPN bridge required by Netty's package-private interface.
Existing Java callers must update imports from `dev.elide.netty.v2`; no legacy
facade is published. C symbols retain `elide_transport_*` and ABI 3 for Cargo and
Elide integration. Maven group changes do not rename that ABI.

Both FFM adapters have a no-argument constructor. Loading uses the defining class
loader's `META-INF/native/<platform>/<mapped-library-name>` resource. Platform
names match the native Maven classifiers: `linux-x86_64-gnu`, `osx-aarch64`, and
corresponding OS/architecture combinations. Aliases such as `amd64`/`x86_64` and
`arm64`/`aarch64` normalize consistently. Classifier recognition does not mean
that every combination is published: CI currently packages Linux glibc x86-64
and macOS ARM64. Linux musl and Windows JVM packaging are not yet qualified.
Unsupported OS/architecture values fail before extraction; a missing classifier
reports the exact resource and Maven dependency to add. Linux selection targets
glibc; it does not silently substitute a glibc library for a published musl variant.

The loader has no Netty, Elide runtime, or GraalVM dependency. It follows Netty's
resource-extraction approach while using JDK FFM for library lookup. It checks
all copies of a matching resource: identical bytes are accepted; conflicting
binaries fail instead of choosing classpath order. Extracted bytes are checked
again after copying. The resources themselves must come from trusted artifacts;
this digest comparison is consistency checking, not publisher authentication.

Extraction is synchronized per defining class loader. Both adapters reuse one
private temporary directory and path. Files remain available until JVM exit,
allowing the metadata adapter to close and reopen its arena while the transport
adapter retains its process-lifetime mapping. Cleanup uses delete-on-exit with
the file ordered before its directory. Windows may keep mapped DLLs locked at
exit, so normal OS temporary-directory cleanup can still be needed there.
Independent class loaders may load independent native registries; do not exchange
native handles across them. Changing the context class loader does not change
which artifact is selected.

Configuration:

| Setting | Effect |
| --- | --- |
| Explicit constructor `FfmTransportNative(Path)` | Uses that library directly; bypasses automatic resource selection |
| `-Dbemo.native.path=/absolute/library/path` | No-argument constructors load this existing file instead of a resource |
| `-Dbemo.native.workdir=/writable/executable/directory` | Parent directory for a private extraction directory |
| `java.io.tmpdir` | Default extraction parent when no Bemo work directory is configured |

Set properties before creating adapters. The resource extraction path is cached;
a later change to the work directory does not relocate an existing mapping.
Choose an executable filesystem when the default temporary filesystem is mounted
`noexec`. The loader does not search `java.library.path`; the explicit override
makes installed-library selection unambiguous. Both paths still validate the ABI
before binding transport operations. Run stock JDK 22+ with
`--enable-native-access=ALL-UNNAMED` on the classpath, as required by the FFM API.

The package verifier checks real JAR-based loading, concurrent extraction,
close/reopen, property overrides, missing and conflicting resources, cleanup,
and TCP/Unix/TLS through the automatically loaded transport. It also links a
separate C program against the **static** classifier; see
[publishing](publishing.md) for Native Image and C linkage instructions.
