# Maven and Cargo delivery

`make package` stages `build/maven/dev/elide/dokar/` and a platform-specific unsigned
ZIP. `python3 tools/verify_package.py` checks metadata, hashes, class version,
runtime isolation, and loading the packaged native library. This is local
staging; no task uploads or publishes anything.

| Artifact | Contents |
| --- | --- |
| `dev.elide.dokar:dokar-api` | Java boundary interface |
| `dev.elide.dokar:dokar-ffm` | FFM adapter; depends only on `dokar-api` |
| `dev.elide.dokar:dokar-netty` | Stock Netty channels, buffer allocator, event-loop and TLS adapters |
| `dev.elide.dokar:dokar-native-image` | C API adapter; API dependency and provided GraalVM SDK dependencies |
| `dokar-ffm:<platform>` classifier | Cargo shared library, C header, and license notices |
| `dokar-native-image:<platform>` classifier | Cargo static library, C header, and license notices |

Every Java artifact includes its POM, sources, and Javadoc. Native classifiers
use `META-INF/native/<platform>/`. Add both the base FFM JAR and the shared-library
classifier JAR to the runtime classpath. For example, on Linux glibc x86-64:

```xml
<dependency>
  <groupId>dev.elide.dokar</groupId>
  <artifactId>dokar-netty</artifactId>
  <version>${dokar.version}</version>
</dependency>
<dependency>
  <groupId>dev.elide.dokar</groupId>
  <artifactId>dokar-ffm</artifactId>
  <version>${dokar.version}</version>
</dependency>
<dependency>
  <groupId>dev.elide.dokar</groupId>
  <artifactId>dokar-ffm</artifactId>
  <version>${dokar.version}</version>
  <classifier>linux-x86_64-gnu</classifier>
  <scope>runtime</scope>
</dependency>
```

`new dev.elide.dokar.transport.FfmTransportNative()` selects and extracts the
library automatically. The metadata binding in `dev.elide.dokar.ffm` supports
the same no-argument constructor. Explicit `Path` constructors remain available.
See [native loading](native-loading.md) for overrides and class-loader behavior.

## Static linkage

Cargo builds **both** a shared and a static library on every release build:
`libdokar_ffi.so`/`.dylib` (Windows `dokar_ffi.dll`) and `libdokar_ffi.a`
(Windows `dokar_ffi.lib`). The static archive is published separately as
`dev.elide.dokar:dokar-native-image:<version>:<platform>`; it is not an import
library for the shared binary. Both `dokar.h` and `elide_transport.h` are included.
There is no dependency on the FFM classifier for static consumers.

Extract `META-INF/native/<platform>/` from that classifier JAR and give Native
Image `-H:CLibraryPath=<directory>` and
`--native-compiler-options=-I<directory>`. Use the base `dokar-native-image` and
`dokar-api` JARs and the GraalVM SDK at build time. Its `@CLibrary(requireStatic =
true)` binding selects the archive. The linked executable needs no Dokar shared
library or runtime resource extraction. This statically links Dokar; it does not
promise a fully static libc/JDK executable.

For a C consumer on Linux, link the extracted archive explicitly, after the
consumer object, and include `-ldl -lpthread -lm`. Platform system libraries are
still required. `python3 tools/verify_package.py` compiles and executes a C
consumer using only the packaged archive and packaged headers, checking both
metadata and transport ownership operations. `make test-native-image` separately
checks the complete statically linked Java transport contracts.

The Maven group is `dev.elide.dokar` and the repository is `elide-dev/dokar`.
Central namespace ownership must be verified before a release.
The initial `.version` is a snapshot, intentionally unsuitable for a Central
release. The transport is extracted, but cross-platform release qualification and the
Linux compatibility floor remain prerequisites for production release.

## Release preparation

1. Set `.version` to the release version and keep Cargo's workspace version in
   sync. Confirm repository, licensing, developer, and SCM metadata.
2. Run the verification graph and review native classifier coverage. Establish
   the Linux glibc floor and test release binaries on each supported platform.
3. Gather the staged repositories from the platform build artifacts. Merge
   native classifiers into one repository; common POMs and Java artifacts should
   be identical. Investigate differing common files instead of overwriting them.
4. Sign each POM and JAR with the project's PGP key using detached armored
   signatures (`gpg --armor --detach-sign <file>`). The stage includes MD5,
   SHA-1, SHA-256, and SHA-512 checksums. Checksums are compatibility metadata;
   PGP signatures establish publisher authenticity.
5. ZIP the merged repository with `dev/` at the archive root, then validate it
   through the Central Publisher Portal under the verified namespace. Publish
   only after reviewing the deployment. Snapshot versions require a snapshot
   repository, not Central's release endpoint.
6. Tag the source revision and record the matching Cargo Git revision and Maven
   version in Elide's dependency declarations. Publish one version across all
   Java artifacts and classifiers.

Signing credentials and Central tokens are not required for builds or tests.
GitHub releases publish the tested, unsigned Maven staging ZIPs with provenance
and Sigstore signatures. Maven Central publication remains a separate operation:
Sigstore signatures do not replace Central's per-JAR/POM PGP signatures.
See [release automation and verification](releases.md).

See [Central's artifact requirements](https://central.sonatype.org/publish/requirements/)
and [Portal upload layout](https://central.sonatype.org/publish/publish-portal-upload/).
