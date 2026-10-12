# Observability

What you can observe today, from the JVM and from Rust. Structured logging,
spans, and metrics with OpenTelemetry bridging are designed but not yet built;
see the [observability design](../superpowers/specs/2026-10-11-transport-observability-design.md).

## Which backend is running

```java
System.out.println(DriverSelection.observed().describe());
```

`DriverSelection.observed()` reports the latest event-loop driver selection,
including the reason AUTO fell back from io_uring to polling, such as a seccomp
policy refusing ring setup. On an owner thread, `TransportNative.driverBackend`
returns the actual backend code and `driverFallback` returns the fallback
reason. A selection is an observation, not a promise about future drivers.

## JFR events

`bemo-netty` defines JFR events in the `Elide / Transport` category. None
records endpoint names or payloads.

| Event | Default | Records |
| --- | --- | --- |
| `dev.elide.TransportTlsHandshake` | Enabled | Role, negotiated protocol, and outcome (success, error, or closed) per handshake |
| `dev.elide.TransportShutdown` | Enabled | Backend, busy retries, status, and bytes still retained after driver release |
| `dev.elide.TransportBatch` | Disabled | One poll and dispatch pass: completions by kind, errors, bytes, and poll and dispatch time |
| `dev.elide.TransportCopy` | Disabled | Managed-write or TLS staging copies, with reason and size |

Enable the disabled events for batching or copy analysis:

```sh
java -XX:StartFlightRecording:settings=profile,dev.elide.TransportBatch#enabled=true ...
```

## Memory

- `TransportNative.ownerUsed(owner)` returns a budget's live native bytes,
  including retained slices.
- `NativeDirectByteBufAllocator.nativeAllocatedBytes()` (and `bemo_alloc_live`)
  return process-wide bytes allocated for Netty direct buffers.
- The `TransportShutdown` event's `retainedBytes` reports storage that a
  released driver could not yet free.

## Driver statistics

Set `ELIDE_TRANSPORT_DIAGNOSTICS=1` to print each driver's backend and
persistent-receive statistics to stderr when it shuts down.
