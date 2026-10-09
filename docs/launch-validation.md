# Public launch validation

The consumer release is **0.3.0** (release source `0a3cc91d1f3683e7fcefc58b5be6f8426fb3bab4`), fetched directly from
[Maven Central](https://repo.maven.apache.org/maven2/dev/elide/bemo/bemo-ffm/0.3.0/).
Current development measurements have a separate revision and are not labeled
as measurements of that release.

## Public release checks

| Check | macOS ARM64 | Linux x86-64 |
| --- | --- | --- |
| Central base POM/JARs: API, FFM, Netty, Native Image | Passed | Passed |
| Central FFM/static classifiers: both advertised platforms | Passed (download) | Passed (download) |
| Maven quickstart, fresh cache and empty settings | Passed | Passed |
| Gradle wrapper quickstart, fresh Gradle home | Passed | Passed |
| HTTP status/body/type and repeated requests | Passed | Passed |
| FFM/backend diagnostic and workload shutdown | Passed | Passed |
| Native Image published static archive ABI/buffer/driver probe | Passed | Passed |
| JVM qualification floors: macOS 15 / glibc 2.39 | Not run | Passed (Ubuntu 24.04 container) |
| JDK 22 consumer execution | Passed (22.0.2, both builders) | Passed (22.0.2, both builders) |

Actual hosts: macOS 26.6.2 ARM64; Linux 7.0.0-34 x86-64 with glibc 2.43.
Both hosts passed with OpenJDK 25.0.2, compiling with `--release 22`.
macOS additionally passed both builders and server checks on OpenJDK 22.0.2;
[minimum-JDK evidence](validation/release-0.3.0-macos-jdk22.json) records that run.
Linux additionally passed both builders in a fresh Ubuntu 24.04 container with
glibc **2.39-0ubuntu8.9** and OpenJDK **22.0.2**, with no host dependency caches
or credentials mounted. [Linux floor evidence](validation/release-0.3.0-linux-glibc239-jdk22.json)
records the image digest, OS, JDK, results, and matching artifact hashes.
The container retains the host's Linux 7.0 kernel. Its policy rejects io_uring
setup with `EPERM`; AUTO reported the reason and selected epoll. This qualifies
the JVM release path at the glibc/JDK floors, not io_uring inside that container
or a Native Image executable built at those floors.
Maven versions were 3.9.15 on macOS and 3.9.11 on Linux; the Gradle wrapper is
9.5.1 with a pinned distribution checksum. Artifact URLs, byte counts, and
SHA-256 digests are retained in [macOS evidence](validation/release-0.3.0-macos.json)
and [Linux evidence](validation/release-0.3.0-linux.json). Downloading the other
platform's classifier is not an execution test of that classifier.

The independent project contains no local-file repository, parent build,
staged Bemo dependency, source-builder invocation, or GraalVM dependency.
The harness copies it to a fresh directory, provides fresh Maven and Gradle
caches, and supplies empty user/global Maven settings. It removes credential
environment variables. Its consumer commands are the
[documented Maven/classpath and Gradle distribution commands](installation.md).
Python orchestrates verification; the consumer build and server do not require it.

Reproduce from the repository root (choose a new output directory):

```sh
python3 tools/verify_release_consumer.py --output build/consumer-verification
```

This verifies all advertised Central coordinates before compiling and serving
HTTP with each builder. It retains exact commands, toolchain details, build and
server logs, and downloaded artifact hashes in that output directory. It does
not stage or publish Bemo. The server uses port 8080; stop another local server
before running the check.

The [Native Image archive probe](publishing.md#static-linkage) was built
separately with the repository's GraalVM SDK-compatible Elide toolchain 2026.10.0
(GraalVM SDK 25.3.4.1), using the downloaded `libbemo_ffi.a` and headers from
0.3.0. Both executables reported `ownership probe passed`; macOS selected backend
1 (kqueue), Linux backend 2 (io_uring). Native Image reports Oracle GraalVM 25.3.4.1+1.1 (Java 25.0.4.1).
The probe's commands invoke only JDK tools,
Native Image, curl, and the C/linker toolchain. No Bemo source compilation or
shared-library extraction occurs. This verifies Bemo static linkage, not a
fully static libc/JDK executable or a release-consumer framework HTTP server.

## Repository and performance checks

The benchmark build at `16805b3bbc01d1ae6bd5d79376a929392a27c483` passed
`make build`, `make check`, `make test`, and `make test-native-image` on Linux.
The examples passed all endpoints, TLS 1.2/1.3, gzip negotiation, concurrent
requests, persistent connections, and native reclamation checks with Bemo and
epoll/tcnative BoringSSL, in JVM and Native Image modes. These use source-built
artifacts, independently of the public-release quickstart checks above.

The [fresh development benchmark report](performance/launch-refresh.md) records
72 full-stack samples, 180 framework samples, and 85 Rust Criterion benchmarks.
Host-native CPU targeting was confined to benchmark preparation in an isolated
checkout; release build defaults are unchanged. All twelve syscall probes passed,
and post-cycle source/artifact/CPU flag hashes match the prepared manifest.

The patch also passes local `make check`, `make test`, `make test-native-image`,
format checks, and the Python tooling tests. `make test` includes the
revision-pinned external Cargo dependency test in `tools/test_git_dependency.py`.

## Remaining launch actions

Recheck the consumer path on **macOS 15** before claiming fresh verification at
that OS floor; only macOS 26.6.2 was available here. Both platforms passed JDK 22,
and Linux passed the glibc 2.39 floor. macOS 15 remains the repository's declared
qualification target, supported by the linked CI matrix rather than this fresh run.

The Spring Boot, Micronaut, and Ktor builds retain contributor staging. They are
labeled source-build examples; their Maven/Gradle paths are not claimed as clean
Central-only release consumers. Framework Native Image builds also retain the
Rust/Elide/Python pipeline. The independent JVM quickstart and static archive
probe supply the verified release paths.

No release publication, repository-setting change, announcement, or external
message is part of this patch. A maintainer should review the refreshed benchmark
evidence and release-versus-HEAD labels before announcing the project. Any
numerical JNI boundary comparison or AWS-LC binary deduplication claim requires
separate controlled work.
