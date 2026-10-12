# Configuration

Bemo has few knobs, and all of them are explicit.

## Native library loading (JVM)

| Setting | Effect |
| --- | --- |
| `new FfmTransportNative(Path)` | Load that library file directly |
| `-Dbemo.native.path=<absolute path>` | No-argument constructors load this file instead of the classpath resource |
| `-Dbemo.native.workdir=<directory>` | Parent of the private directory where the classpath library is extracted |
| `java.io.tmpdir` | Default extraction parent |

Set these before creating a binding. Choose an executable filesystem when the
default temporary directory is mounted `noexec`. The loader never searches
`java.library.path`. Details are in [native loading](../native-loading.md).

## Event-loop and driver limits

`NativeIoHandler.newFactory(transport, backend, limit, memoryLimit)`:

| Argument | Values |
| --- | --- |
| `backend` | `0` AUTO, `1` polling, `2` io_uring, `3` IOCP |
| `limit` | 1 to 65,536 concurrent operations per driver |
| `memoryLimit` | At least `limit * 40` bytes of native buffer budget per handler |

AUTO falls back from io_uring to polling only when ring setup fails, and
records why (see [observability](observability.md)). An explicit backend that
cannot start fails instead of falling back.

## Environment variables

| Variable | Effect |
| --- | --- |
| `ELIDE_TRANSPORT_DIAGNOSTICS=1` | Print each driver's backend and persistent-receive statistics to stderr when it shuts down |
| `ELIDE_TRANSPORT_TEST_BACKEND` | Test suites only: force `io-uring`, `epoll`, or `kqueue` for drivers the tests create |

## Rust features

See [Rust](rust.md#features) for allocator selection (`bundled-mimalloc`,
`rust-allocator`, or an embedder-provided `mi_malloc`).
