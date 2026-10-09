# Install the published JVM release

Use **Bemo 0.3.0** from Maven Central with **JDK 22+** and **Netty 4.2**.
The JVM API and FFM artifacts require neither GraalVM nor the Elide runtime.
Use the classpath for Netty TLS; named JPMS modules are not supported there.

| Release classifier | Qualification target | Selection |
| --- | --- | --- |
| `linux-x86_64-gnu` | Linux x86-64, glibc 2.39 | glibc Linux on AMD64 |
| `osx-aarch64` | macOS 15, ARM64 | Apple Silicon macOS |

These targets match the [packaging CI matrix](../.github/workflows/job.build.yml)
and [JVM contract matrix](../.github/workflows/check.jvm.yml). macOS packaging
sets `MACOSX_DEPLOYMENT_TARGET=15.0`; Linux packaging validates glibc.
They are qualification targets, not proof that every older OS works. The
fresh consumer checks in [launch validation](launch-validation.md) identify the
actual tested environments. No release package is qualified here for Windows,
musl/Alpine, Linux ARM64, or Intel macOS. Possible source builds have a different
scope. Contributor builds use JDK 25; consumers need only JDK 22+.

The [README dependency snippets](../README.md#usage) include all three required
coordinates: `bemo-netty`, the base `bemo-ffm` JAR, and the platform-specific
`bemo-ffm` classifier. The latter contains the shared library, loaded automatically.
Select the classifier for the machine running the application.

## Run a complete server

[The standalone quickstart](../examples/quickstart) uses only published 0.3.0
artifacts from Central. Copy that directory into a clean project, or clone the
repository and enter `examples/quickstart`. Its build files and source do not
read anything outside that directory. No Rust, Elide, Python, staging step,
GitHub credentials, or local Bemo JAR is required.

The example serves plaintext HTTP/1.1 over Bemo sockets, retaining Netty's HTTP
codecs. It does not enable TLS, gzip, a stock comparison toggle, or Native Image.

### Maven

Linux x86-64:

```sh
cd examples/quickstart
mvn -Dbemo.classifier=linux-x86_64-gnu package dependency:build-classpath \
  -Dmdep.outputFile=target/runtime.classpath
java --enable-native-access=ALL-UNNAMED \
  -cp "target/classes:$(cat target/runtime.classpath)" \
  dev.elide.bemo.examples.quickstart.Application
```

On Apple Silicon, replace `-Dbemo.classifier=linux-x86_64-gnu` with
`-Dbemo.classifier=osx-aarch64`. Maven includes Central by default.

### Gradle wrapper

Linux x86-64:

```sh
cd examples/quickstart
./gradlew -Pbemo.classifier=linux-x86_64-gnu installDist
build/install/bemo-quickstart/bin/bemo-quickstart
```

On Apple Silicon, use `-Pbemo.classifier=osx-aarch64` instead. The generated
launcher supplies `--enable-native-access=ALL-UNNAMED`. Alternatively,
`./gradlew -Pbemo.classifier=osx-aarch64 run` starts the application directly.

### Check and stop

From another terminal:

```sh
curl -i http://127.0.0.1:8080/plaintext
```

Expect status `200`, `Content-Type: text/plain`, and body `Hello, World!`.
Startup prints `Bemo FFM` and the observed backend. Ctrl-C closes the server,
stops its event loops, and releases the default workload; it prints
`Bemo stopped; workload released`.

One Java API works with different platform binaries. Linux AUTO tries io_uring
and may fall back to epoll; macOS uses kqueue. Inspect
`DriverSelection.observed().describe()` after the loops start to see the selected
backend and the io_uring failure reason, when present. An explicit backend
request fails instead of falling back. See [architecture](architecture.md#io-backends).

## Other integration paths

The [framework examples](framework-examples.md) retain their contributor staging
workflow and expose TLS, gzip, and a native Netty comparison. They are source-build
examples, not the independent release quickstart above.

Native Image uses a separate binding and a published static archive. See
[static linkage](publishing.md#static-linkage) for the required GraalVM and C/linker
toolchain and classifier extraction. Those instructions link the published Bemo
archive; compiling Bemo's Rust sources is unnecessary for a release consumer.
Framework Native Image examples still use the contributor build pipeline.
