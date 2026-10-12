# Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `IllegalCallerException` or a warning about restricted methods | FFM native access is not enabled | Run with `--enable-native-access=ALL-UNNAMED` |
| An error naming a missing `META-INF/native/<platform>/` resource | The platform classifier JAR is not on the runtime classpath | Add `bemo-ffm:<version>:<platform>`, or set `bemo.native.path` |
| An unsupported platform error before extraction | No classifier is published for this OS, architecture, or libc | See [platform support](overview.md#platform-support); musl and Windows JVM packages are not published |
| Extraction fails or the library cannot be mapped | The temporary directory is `noexec` | Set `-Dbemo.native.workdir` to an executable directory |
| `LinkageError: Bemo ABI mismatch` | Java artifacts and the native library come from different versions | Use one Bemo version for every artifact |
| AUTO runs polling instead of io_uring | io_uring setup was refused, often by seccomp (`-1001`, EPERM) or an older kernel (`-1022`, EINVAL) | Check `DriverSelection.observed().describe()`; allow `io_uring_setup` or request polling explicitly |
| An explicit io_uring backend fails to start | Same as above; explicit backends never fall back | Use AUTO, or fix the environment |
| TLS classes fail to load in a modular application | Netty's ALPN interface is package-private | Run TLS on the classpath, not the module path |
| Duplicate symbol errors linking a native image or embedder | Both the shared and static Bemo libraries, or two `aws-lc-sys` versions, are linked | Link one Bemo library; keep `aws-lc-rs` on Bemo's version (see [embedding](../elide-integration.md)) |
| `UnsupportedOperationException: Bemo native allocation is unavailable` | The loaded library predates `CAP_NATIVE_ALLOC` | Upgrade the native library with the Java artifacts |
| Allocation failures under load | A handler's `memoryLimit` budget is exhausted | Raise `memoryLimit`, or apply backpressure earlier |

For a reproducible bug report, include `DriverSelection.observed().describe()`,
the kernel version, and a JFR recording with `dev.elide.TransportBatch` enabled.
