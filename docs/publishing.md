# Maven and Cargo delivery

`make package` stages `build/maven/dev/elide/bemo/` and a platform-specific unsigned
ZIP. `python3 tools/verify_package.py` checks metadata, hashes, class version,
runtime isolation, and loading the packaged native library. This is local
staging; no task uploads or publishes anything.

| Artifact | Contents |
| --- | --- |
| `dev.elide.bemo:bemo-api` | Java boundary interface and JSpecify nullness annotations |
| `dev.elide.bemo:bemo-ffm` | FFM adapter; depends on `bemo-api` (which exposes JSpecify) |
| `dev.elide.bemo:bemo-netty` | Stock Netty channels, buffer allocator, event-loop and TLS adapters |
| `dev.elide.bemo:bemo-native-image` | C API adapter; API dependency and provided GraalVM SDK dependencies |
| `bemo-ffm:<platform>` classifier | Cargo shared library, C header, and license notices |
| `bemo-native-image:<platform>` classifier | Cargo static library, C header, and license notices |

Every Java artifact includes its POM, sources, and Javadoc. Native classifiers
use `META-INF/native/<platform>/`. Add both the base FFM JAR and the shared-library
classifier JAR to the runtime classpath. For example, on Linux glibc x86-64:

```xml
<dependency>
  <groupId>dev.elide.bemo</groupId>
  <artifactId>bemo-netty</artifactId>
  <version>${bemo.version}</version>
</dependency>
<dependency>
  <groupId>dev.elide.bemo</groupId>
  <artifactId>bemo-ffm</artifactId>
  <version>${bemo.version}</version>
</dependency>
<dependency>
  <groupId>dev.elide.bemo</groupId>
  <artifactId>bemo-ffm</artifactId>
  <version>${bemo.version}</version>
  <classifier>linux-x86_64-gnu</classifier>
  <scope>runtime</scope>
</dependency>
```

`new dev.elide.bemo.transport.FfmTransportNative()` selects and extracts the
library automatically. The metadata binding in `dev.elide.bemo.ffm` supports
the same no-argument constructor. Explicit `Path` constructors remain available.
See [native loading](native-loading.md) for overrides and class-loader behavior.

## Static linkage

Cargo builds **both** a shared and a static library on every release build:
`libbemo_ffi.so`/`.dylib` (Windows `bemo_ffi.dll`) and `libbemo_ffi.a`
(Windows `bemo_ffi.lib`). The static archive is published separately as
`dev.elide.bemo:bemo-native-image:<version>:<platform>`; it is not an import
library for the shared binary. Both `bemo.h` and `elide_transport.h` are included.
There is no dependency on the FFM classifier for static consumers.

Extract `META-INF/native/<platform>/` from that classifier JAR and give Native
Image `-H:CLibraryPath=<directory>` and
`--native-compiler-options=-I<directory>`. Use the base `bemo-native-image` and
`bemo-api` JARs and the GraalVM SDK at build time. Its `@CLibrary(requireStatic =
true)` binding selects the archive. The linked executable needs no Bemo shared
library or runtime resource extraction. This statically links Bemo; it does not
promise a fully static libc/JDK executable.

For a C consumer on Linux, link the extracted archive explicitly, after the
consumer object, and include `-ldl -lpthread -lm`. Platform system libraries are
still required. `python3 tools/verify_package.py` compiles and executes a C
consumer using only the packaged archive and packaged headers, checking both
metadata and transport ownership operations. `make test-native-image` separately
checks the complete statically linked Java transport contracts.

The Maven group is `dev.elide.bemo` and the repository is `elide-dev/bemo`.
The current Maven registry is GitHub Packages at
`https://maven.pkg.github.com/elide-dev/bemo`.
Central namespace ownership must be verified before a release.
Central accepts stable release versions; snapshot publication stays in GitHub
Packages. Qualification targets glibc 2.39/Linux x86-64 and macOS 15/ARM64;
packaging verifies binary requirements and CI runs consumers on those builders.
The first signed release and its consumer provenance verification still require
a successful merged-source release run.

## GitHub Packages

After a main push passes verification, `job.packages.yml` downloads the tested
Linux and macOS bundles from that same run and verifies their build attestations.
It merges their Maven repositories, requiring common Java artifacts to be
identical, and publishes all four modules with sources, Javadocs, and both native
classifiers. Maven's pinned deploy plugin only uploads existing artifacts; Cargo
and Elide continue to own all compilation and JAR generation. Publication then
downloads every POM and JAR and checks it against the verified staged bytes.

Snapshot versions publish on verified main pushes using Maven's timestamped
snapshot metadata. Stable versions publish only from the main revision identified
by their release tag; later development commits do not overwrite that version.
The publishing job follows the GitHub release job and uses the release environment
and `GITHUB_TOKEN` with `packages: write`. PR verification receives no publishing
credentials. Maven Central remains a separate publishing step.

Consumers add this repository alongside Central:

```xml
<repository>
  <id>github</id>
  <url>https://maven.pkg.github.com/elide-dev/bemo</url>
  <snapshots><enabled>true</enabled></snapshots>
</repository>
```

GitHub's Maven registry requires authentication for public packages too. Configure
the matching server in Maven settings using a classic token with `read:packages`,
or use a workflow token granted access to this package:

```xml
<server>
  <id>github</id>
  <username>${env.GITHUB_USER}</username>
  <password>${env.GITHUB_TOKEN}</password>
</server>
```

See [GitHub's Maven registry documentation](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-apache-maven-registry)
for authentication and repository access.

## Maven Central

Use the Central Publisher Portal API, keeping Cargo and Elide as the producers
of the artifacts. The retired OSSRH endpoints and a Maven compilation lifecycle
are unnecessary here. `tools/publish_central.py` reuses the checked platform
merge, signs the existing artifacts, and exposes separate `prepare`, `upload`,
`status`, `publish`, and `verify` operations. See the
[Publisher API](https://central.sonatype.org/publish/publish-portal-api/).

Before the first release, a maintainer must verify the `dev.elide.bemo` namespace
(or an owning parent namespace) in the Portal, generate a Portal user token,
and make the signing key's public key available on a
[Central-supported key server](https://central.sonatype.org/publish/requirements/gpg/).
Keep the private signing key and token outside the repository and PR workflows.
The Portal token is distinct from a GitHub Packages token.

1. Set `.version` to the stable version and keep Cargo's workspace version in
   sync. Review the POM licensing, developer, and SCM metadata.
2. Complete the merged-main verification graph and gather both tested platform
   bundles and their provenance files into `build/release-input`. Set
   `GITHUB_REPOSITORY=elide-dev/bemo` and `GITHUB_SHA` to that tested revision,
   then run `python3 tools/release.py stage` to verify its main-source
   attestations. Use the release workflow's staged output, or download its
   matching build artifacts; do not mix runs or rebuild for publication.
3. With the project's PGP key loaded into GPG, prepare the signed bundle:

   ```sh
   python3 tools/publish_central.py prepare --key "$BEMO_SIGNING_KEY"
   ```

   `BEMO_SIGNING_KEY` is the signing key fingerprint, not key material. GPG
   uses its normal agent/pinentry for unlocking the key. Preparation merges
   both platforms, rejects differing common files, signs and verifies every
   POM/JAR, regenerates MD5/SHA-1/SHA-256/SHA-512 checksums, and writes
   `build/central/bemo-<version>-central.zip` with `dev/` at its root. It
   requires no Portal credentials and makes no network requests.
4. Supply `CENTRAL_TOKEN_USERNAME` and `CENTRAL_TOKEN_PASSWORD` through your
   secret manager or protected publishing environment, then upload:

   ```sh
   python3 tools/publish_central.py upload
   python3 tools/publish_central.py status --deployment "$BEMO_DEPLOYMENT_ID"
   ```

   Save the UUID printed by `upload` as `BEMO_DEPLOYMENT_ID`. The upload uses
   `USER_MANAGED`, so validation does not publish. Inspect status errors in
   the Portal; poll `status` until `VALIDATED`. Review all four modules,
   sources, Javadocs, and ordinary/ThinLTO native classifiers together.
5. After reviewing the validated deployment, publish through the Portal UI or:

   ```sh
   python3 tools/publish_central.py publish --deployment "$BEMO_DEPLOYMENT_ID"
   python3 tools/publish_central.py status --deployment "$BEMO_DEPLOYMENT_ID"
   ```

   `publish` rejects any state other than `VALIDATED`. Continue checking status
   until `PUBLISHED`, then run `python3 tools/publish_central.py verify` to
   compare every public POM/JAR with the signed bundle. The POST acknowledges
   a publication request, not its completion. A released version is immutable. Preserve the signed ZIP and
   deployment ID with the source revision before announcing availability.

`--version`, `--source`, and `--bundle` support an explicit staged release;
the default version comes from `.version`. Only stable `major.minor.patch`
versions are accepted. Local GPG verification requires the signing public key
in the local keyring. No Central upload or publication is wired into main/PR
verification. Namespace verification, Portal validation, and the first live
publication still require the maintainer's account and credentials.

GitHub release Sigstore signatures authenticate the staged ZIPs and provenance;
they do not replace Central's per-POM/JAR PGP signatures. See
[artifact requirements](https://central.sonatype.org/publish/requirements/),
[Portal bundle layout](https://central.sonatype.org/publish/publish-portal-upload/),
and [release automation](releases.md).

Native Image additionally stages `<platform>-thinlto` classifiers. Their
archive, target contracts, and manifest are verified from the staged JAR through
a real ThinLTO ownership consumer. Cross-platform merging and deployment include
these variants alongside the ordinary classifiers; common Java artifacts stay
platform-independent. Staging requires LLVM/Clang/LLD compatible with the pinned
Rust compiler. See [generated seams](generated-seam.md).
