# GraalVM Native Image

In a native image, Bemo is linked statically and called through GraalVM's C
interface instead of FFM. The Netty classes are the same as on the JVM; only the
binding changes:

```java
var transport = new dev.elide.bemo.transport.svm.CapiTransportNative();
```

## Dependencies and linking

Use `bemo-netty`, `bemo-api`, and `bemo-native-image` at build time, plus the
static classifier `dev.elide.bemo:bemo-native-image:<version>:<platform>`. That
classifier holds `libbemo_ffi.a` and both headers under
`META-INF/native/<platform>/`. Extract it and point Native Image at it using an
absolute path, because the C compiler runs from a temporary directory:

```sh
native_dir="$PWD/archive/META-INF/native/linux-x86_64-gnu"
native-image -H:CLibraryPath="$native_dir" \
  --native-compiler-options=-I"$native_dir" \
  -H:NativeLinkerOption=-ldl -H:NativeLinkerOption=-lpthread -H:NativeLinkerOption=-lm \
  ...
```

The binding's `@CLibrary(requireStatic = true)` selects the archive, so the
executable needs no shared library and does no resource extraction at run time.
This links Bemo statically; it does not by itself make the whole executable
static. The [release probe](../../examples/native-probe) downloads a published
release and links it end to end.

`bemo-netty` carries Native Image configuration that enables shared arenas,
which Netty's Java 25 direct-buffer cleaner needs even when transport calls use
the C interface.

## Differences from the JVM binding

- Netty's `PlatformDependent` usually has no `Unsafe` in a native image, so
  Netty uses its NIO-backed buffers. Bemo's buffer views are big-endian, as
  those buffers require.
- Native Image executables cannot run under ASAN or TSAN. Bemo's sanitizer runs
  cover the same Rust code through C consumers and the FFM binding instead.
- JFR events work when the image is built with `--enable-monitoring=jfr`.

## Fully static executables

Elide builds fully static musl executables with the Elide toolchain. Bemo
qualifies that shape through its embedder contract rather than through
published classifiers; see [Embedding Bemo in Elide](../elide-integration.md).
