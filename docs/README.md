# Bemo documentation

## Using Bemo

The [user guide](guide/README.md) is the place to start: an overview, then Netty
on the JVM, Native Image, Rust, ABI concepts, configuration, observability, and
troubleshooting. It is written to be imported into Elide's documentation as is.

- [Installation](installation.md): a complete server from Maven Central.
- [Framework examples](framework-examples.md): Spring Boot, Micronaut, and Ktor.
- [Native HTTP workload](native-http-example.md): the C ABI's HTTP server path.
- [Embedding Bemo in Elide](elide-integration.md): the embedder contract.
- [C ABI reference](reference/c-abi.md): generated from the public headers.

## How Bemo works

- [Architecture](architecture.md): components, backends, TLS, HTTP, and memory.
- [I/O ownership and batching](transport-io.md): send, receive, and shutdown rules.
- [Native loading](native-loading.md): Java namespaces and library extraction.
- [Generated seams](generated-seam.md): Native Image imports and ThinLTO archives.
- [Observability design](superpowers/specs/2026-10-11-transport-observability-design.md) (proposed).

## Verifying Bemo

- [Native safety](native-safety.md): sanitizers, Miri, and fuzzing.
- [Measurement](measurement.md): tests, coverage, and continuous benchmarks.
- [Checks](checks.md): strictness, pinned tools, and hooks.
- [CI](ci.md): the verification graph.
- [Performance reports](performance/README.md).

## Releasing Bemo

- [Publishing](publishing.md): Maven and Cargo delivery, static linkage.
- [Releases](releases.md): Release Please, provenance, and signing.
- [Launch validation](launch-validation.md).

## History

[Extraction](extraction.md) and [rename](rename.md) record how Bemo was split
out of Elide. [The work plan](work-plan.md) is the running engineering log.
