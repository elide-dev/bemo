# Bemo user guide

Bemo is Netty's transport, TLS, and compression layer reimplemented in Rust,
usable from the JVM (through FFM), from GraalVM Native Image (through static
linking), and from Rust directly. This guide is for people building on Bemo. For
how Bemo itself is built and verified, see the [engineering docs](../README.md).

Each page stands alone and links only within this guide or to the reference, so
the set can be imported into another documentation site in this order:

1. [Overview](overview.md): what Bemo replaces, the integration paths, and platform support.
2. [Netty on the JVM](netty.md): event loops, channels, TLS, compression, buffer allocators, and shutdown.
3. [GraalVM Native Image](native-image.md): the static binding and its link requirements.
4. [Rust](rust.md): the core crate, its features, and the TLS and HTTP state machines.
5. [ABI concepts](abi-concepts.md): handles, ownership, threads, workloads, errors, and capabilities.
6. [Configuration](configuration.md): every property, environment variable, and limit.
7. [Observability](observability.md): driver selection, JFR events, and diagnostics.
8. [Troubleshooting](troubleshooting.md): symptoms, causes, and fixes.
9. [Embedding Bemo in Elide](../elide-integration.md): the embedder contract.

Reference:

- [C ABI reference](../reference/c-abi.md), generated from the public headers.
- [Installation](../installation.md): a complete runnable server from Maven Central.
- [Framework examples](../framework-examples.md): Spring Boot, Micronaut, and Ktor.
- [Performance reports](../performance/README.md).
