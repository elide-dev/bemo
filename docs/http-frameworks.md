# Native HTTP framework adapters

The Bali integration candidate adds a shared HTTP/1.1 loop and optional Ktor and
Spring WebFlux adapters. These modules are local candidates; release performance
and lifecycle qualification are still in progress. The Rust seam and generated
image imports remain unchanged.

Bemo parses HTTP wire bytes and encodes response headers and framing in Rust.
The Java loop consumes parsed native events and span metadata, materializes the
framework request, and hands response records back to the native encoder. Ktor
and Spring retain application routing and body codecs; neither adapter runs a
second HTTP wire parser.

`NativeHttpServer` lives in `bemo-api`. Drivers, exchange handles and delivered
body segments stay on its thread. Request publishers copy a segment into managed
storage only when a subscriber requests it, then release the native segment and
its receive-window credit. Response publishers receive one item of demand at a
time; large items are split into native parts, and the next item is requested
after write completion. A response's `sent()` stage completes after transmission
or fails on disconnect, cancellation or server retirement. The server bounds
native storage, concurrent exchanges and total request-body bytes. The initial
implementation has one event loop per listener.

`bemo-spring` supplies `BemoHttpHandlerAdapter` and
`BemoReactiveWebServerFactory`. A Boot application can register the factory as its
`ReactiveWebServerFactory` bean, leaving controllers, message readers/writers and
exception handlers unchanged. Select a compatible backend with
`TransportNative.load()` and retain the stock factory when it returns null. The
factory accepts ordinary Boot address/port and a single TLS server identity from
its SSL bundle. Custom protocol/cipher policy, client authentication and SNI
identities are explicitly refused until supported. HTTP/2, upgrades, compression
configuration and graceful drain are not qualified on this candidate.

`bemo-ktor` supplies `dev.elide.bemo.ktor.Bemo`, an
`ApplicationEngineFactory`. Use `embeddedServer(Bemo)` with the same application
module. It uses Ktor's default engine pipeline, request transformations, routing,
suspending handlers and outgoing content implementations. Optional PEM certificate
and private-key bytes enable native TLS. An unavailable native backend is refused;
an application's engine selector must retain its stock engine as fallback.
Upgrades and HTTP/2 continue to require the stock engine. Standard Ktor SSL
connector configuration and graceful drain remain to be completed.

All framework types remain in the application's class loader. `bemo-bali` selects
Bali's primitive image boundary; `bemo-ffm` supplies a stock-JVM backend. The image
API is provided at runtime and its compile-only stub is never packaged. Neither
native HTTP adapters nor the shared loop import Native Image types or Netty
transport internals.

The shared loop contract covers empty and large bodies, fixed and chunked framing,
HEAD, malformed requests, verified TLS and full native-resource reclamation.
Bali's framework fixtures additionally prove the original controller/routing
responses with the direct backend selected. Throughput samples on an image built
with debug Rust archives are diagnostic only; acceptance requires optimized
archives/images and matched native stock-JVM controls.

Request-head decoding copies the ABI 3 exchange layout, span table and complete head
into managed snapshots before invoking the application. Field strings decode from
those snapshots, preserving header order, duplicate and empty values, Latin-1 text,
the exact request target and HTTP version. This avoids a native view and byte-array
copy for each field; request objects retain no borrowed native memory. Serving
performance still requires a paired measurement.

The HTTP server also accepts an opt-in worker count. Ktor exposes `nativeWorkers`
and Spring's factory exposes `setNativeWorkers`; both default to one. One listener
hands accepted sockets to workers round-robin using the portable adoption API.
Established connections, native drivers, request state and response publishers stay
on their owning worker. The native memory budget and maximum active exchanges are
shared across the server, rather than multiplied by its worker count. Startup
reports one bound port, including port zero; shutdown retires queued accepts and
all worker drivers before releasing the shared workload. A worker startup/runtime
failure stops the complete group. Multi-worker performance remains unqualified.

Ktor's `dispatchCallsOnNativeWorker` option defaults to false. When enabled, a cached
coroutine dispatcher uses the request's `executor()` to queue application work on
its connection owner, including resumptions after suspension. Blocking application
code then occupies that transport worker. Executors reject submissions once shutdown
starts; already accepted callbacks drain before the worker's stopped stage completes.
Synthetic request objects without a native owner cannot supply an executor. This
option remains experimental until differential and serving checks qualify it.

### Complete response transmission

`NativeHttpServer.fixedBody(bytes)` identifies a known body whose bytes remain unchanged
until `Response.sent()` completes. Bodies up to the native response-part limit use one
HTTP/1 encode and one physical completion when metadata capability
`CAP_HTTP1_COMPLETE_CREDIT` (bit `4`) is present. Bit `2` remains reserved for
upstream native allocation; it does not imply complete-response support. Legacy backends and larger or length-mismatched
bodies retain publisher streaming. Ktor byte responses and Spring single-buffer `Mono`
responses use this path; arbitrary publishers retain incremental demand.

The optional transport flag `RESPOND_COMPLETE_CREDIT` is incompatible with streaming
and HTTP/2. Cancellation suppresses reports while retaining native wire leases until
retirement. ABI versions and existing flag behavior remain unchanged.

Ktor cleanup checks whether a byte channel is already closed before invoking its
cancellation helper, avoiding an otherwise unused exception stack trace after success.
Open channels still receive cancellation.
