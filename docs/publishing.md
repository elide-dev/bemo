# Maven and Cargo delivery

`make package` stages `build/maven/dev/elide/` and a platform-specific unsigned
ZIP. `python3 tools/verify_package.py` checks metadata, hashes, class version,
runtime isolation, and loading the packaged native library. This is local
staging; no task uploads or publishes anything.

| Artifact | Contents |
| --- | --- |
| `dev.elide:dokar-api` | Java boundary interface |
| `dev.elide:dokar-ffm` | FFM adapter; depends only on `dokar-api` |
| `dev.elide:dokar-native-image` | C API adapter; API dependency and provided GraalVM SDK dependencies |
| `dokar-ffm:<platform>` classifier | Cargo shared library, C header, and license notices |
| `dokar-native-image:<platform>` classifier | Cargo static library, C header, and license notices |

Every Java artifact includes its POM, sources, and Javadoc. Native classifiers
use `META-INF/native/<platform>/`. The current adapter takes an explicit library
path, so applications must extract the appropriate shared library before FFM
loading. For Native Image, extract the static library and header, then pass
`-H:CLibraryPath=<directory>` and `--native-compiler-options=-I<directory>` to
the image builder. `make test-native-image` demonstrates static linking directly
from the Cargo output and repository header.

The proposed group and repository are `dev.elide` and `elide-dev/dokar`; confirm
the actual hosted repository and Central namespace ownership before release.
The initial `.version` is a snapshot, intentionally unsuitable for a Central
release. The scaffold has no usable Netty transport and should not be presented
as one in release metadata.

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
The current workflows deliberately stop at unsigned staging. A future publishing
workflow can consume these verified artifacts and use a protected environment;
it must never rebuild from untrusted PR code with publishing credentials.

See [Central's artifact requirements](https://central.sonatype.org/publish/requirements/)
and [Portal upload layout](https://central.sonatype.org/publish/publish-portal-upload/).
