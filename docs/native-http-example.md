# Native HTTP server workload

[`NativeHttpBenchmarkServer`](../benchmarks/java/NativeHttpBenchmarkServer.java)
is the runnable native HTTP server used by the full-stack benchmark. This is a
**contributor workload**, distinct from the [published JVM quickstart](installation.md).
It exercises native HTTP/1.1 parsing/response encoding through `TransportNative`,
with optional Rustls/aws-lc-rs TLS and zlib-rs gzip. Framework benchmarks retain
their Java HTTP codecs instead.

With the [contributor toolchain](../README.md#working-on-bemo), run:

```sh
make bench-prepare
python3 tools/bench.py run --case plain-identity-1024
python3 tools/bench.py run --case tls-gzip-131072
```

Preparation builds the optimized statically linked Native Image server. Each
run starts the server, discovers its loopback port, validates every response
with the external JVM client, and shuts it down. Three samples compare Bemo
against the platform's native Netty transport; Netty TLS uses tcnative/BoringSSL.
Linux preparation additionally needs OpenSSL headers for the benchmark client
policy shim. Logs and JSON evidence are under `build/reports/benchmarks`.

This workload verifies an HTTP/1.1 server with persistent connections and the
requested fixed body. Its TLS ALPN explicitly selects `http/1.1`. It does not
provide an HTTP/2 or native HTTP client quickstart. Core HTTP/2 capabilities and
contracts have a broader scope than this example. See
[measurement](measurement.md) for runtime, ownership, and comparison controls.
