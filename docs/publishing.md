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

### Publish an existing release

Central publication uses the bytes from an **immutable GitHub release**. Choose
its stable version; a newer development checkout does not replace that release's
artifacts. To include unreleased code, create a new GitHub release first.

```sh
central_version=0.2.0
python3 tools/publish_central.py fetch --version "$central_version" \
  --source build/central/release
python3 tools/publish_central.py prepare --version "$central_version" \
  --source build/central/release --key "$BEMO_SIGNING_KEY" --without-thinlto
```

`fetch` checks the published release's immutability, resolves its source tag,
verifies GitHub asset digests and the Sigstore-signed checksum list, and verifies
both platform build attestations against that exact main-source commit and
hosted build workflow. It records the release URL, commit and checksums in
`build/central/release/central-release.json`. It does not compile or rebuild.

The original `0.2.0` release has ordinary native classifiers, without ThinLTO.
`--without-thinlto` prepares that historical layout. Omit it for newer releases
with ThinLTO; incomplete classifier pairs are rejected. The default package
build and GitHub Packages publisher continue to require ThinLTO classifiers.

`prepare` merges both platforms, rejects differing common files, signs and
verifies every POM/JAR, regenerates MD5/SHA-1/SHA-256/SHA-512 checksums, and writes
`build/central/bemo-<version>-central.zip` with `dev/` at its root. It requires
no Portal credentials. The PGP key fingerprint is selected with `--key`; local
GPG uses its agent/pinentry. In automation, `BEMO_PGP_PASSPHRASE` supplies the
passphrase to GPG through standard input, never command arguments.

Set `CENTRAL_TOKEN_USERNAME` and `CENTRAL_TOKEN_PASSWORD` from a **Central Portal
user token**, then upload and wait for validation:

```sh
python3 tools/publish_central.py upload --version "$central_version"
# Save the returned UUID as BEMO_DEPLOYMENT_ID.
python3 tools/publish_central.py wait --deployment "$BEMO_DEPLOYMENT_ID" --state VALIDATED
python3 tools/publish_central.py publish --deployment "$BEMO_DEPLOYMENT_ID"
python3 tools/publish_central.py wait --deployment "$BEMO_DEPLOYMENT_ID" --state PUBLISHED
python3 tools/publish_central.py verify --version "$central_version"
```

Uploads use `USER_MANAGED`; `publish` requires `VALIDATED`. `wait` reports Portal
validation errors and times out after 30 minutes by default (`--timeout` adjusts
that limit). `status` remains available for one-shot checks. Final verification
compares every public POM/JAR with the signed bundle. Preserve the ZIP,
`central-release.json` and deployment UUID together. A published version is
immutable; a successful publication POST is not proof that consumers can resolve
it. Use `--bundle` to select a saved ZIP explicitly.

### GitHub Actions

The separate [Publish Maven Central workflow](../.github/workflows/on.central.yml)
is dispatched from `main` for an existing immutable release:

```sh
gh workflow run on.central.yml --ref main -f version=0.2.0
```

Configure these in the protected GitHub `release` environment:

| Setting | Type | Contents |
| --- | --- | --- |
| `CENTRAL_TOKEN_USERNAME` | Secret | Portal token username |
| `CENTRAL_TOKEN_PASSWORD` | Secret | Portal token password |
| `BEMO_PGP_PRIVATE_KEY` | Secret | ASCII-armored private signing key |
| `BEMO_PGP_PASSPHRASE` | Secret | Signing-key passphrase; empty for an unprotected key |
| `BEMO_SIGNING_KEY` | Variable | Full signing-key fingerprint |

The workflow authenticates the release artifacts, signs them, uploads a
user-managed deployment, waits for validation, requests publication, waits for
`PUBLISHED`, and checks the public bytes. It preserves the signed bundle, source
manifest, deployment UUID and state logs as `central-<version>-<run-id>` for
90 days, including on failure. It deletes the imported private key afterward.
PR and main verification do not receive these credentials or run this job.

If validation or publication fails after upload, use the saved deployment UUID
with `status`, `wait`, and `publish` to resume rather than uploading a duplicate
deployment. Namespace verification and a valid Portal token remain account setup
requirements; the workflow cannot establish namespace ownership on your behalf.

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
