# Netty adapter migration boundary

The stock Netty 4.2 channel/event-loop adapter will live here. No working Dokar
channel factory is advertised until it is migrated and tested against Central's
Netty artifacts. API, FFM, and Native Image packages do not depend on Netty.

Migrate Elide's `packages/base/main/dev/elide/netty/v2` channel, buffer, and TLS
adapters, excluding `NativeRegion` (Truffle interop). Preserve reference counting,
shutdown ordering, callback reentrancy, and allocator ownership tests. See
[the extraction guide](../../docs/extraction.md).
