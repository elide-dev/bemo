# Netty on the JVM

This page covers configuring a Netty 4.2 application to use Bemo on JDK 22+.
For dependency coordinates and a complete runnable server, start with the
[installation guide](../installation.md). Run the JVM with
`--enable-native-access=ALL-UNNAMED`.

## Load the native library

```java
var transport = new FfmTransportNative(); // dev.elide.bemo.transport
```

The no-argument constructor loads the shared library from the platform
classifier JAR on the classpath. Set `bemo.native.path` to load a specific file
instead (see [configuration](configuration.md)). Create one `FfmTransportNative`
per process and share it. Native handles are only meaningful to the binding that
created them.

## Event loops

Each `NativeIoHandler` owns one native driver on its event-loop thread:

```java
var group = new MultiThreadIoEventLoopGroup(
    threads, NativeIoHandler.newFactory(transport, backend, limit, memoryLimit));
```

| Parameter | Meaning |
| --- | --- |
| `backend` | `0` AUTO, `1` polling (epoll/kqueue), `2` io_uring, `3` IOCP. AUTO tries io_uring on Linux and falls back to polling if ring setup fails; an explicit backend fails instead of falling back. |
| `limit` | Concurrent native operations per driver, 1 to 65,536. |
| `memoryLimit` | Allocation budget, in bytes, of the native owner each handler creates for its receive and send buffers; at least `limit * 40`. Exhausting it fails allocations, not the process. |

Drivers and their channels belong to the default workload. Pass a
`Workload.Profile` to `newFactory` to place them in another workload, or use
`NativeIoHandler.forWorkload(transport, backend, limit, workload)` to share an
explicit workload's allocation ceiling across drivers. A workload groups work for
cancellation and accounting: closing one cancels its own operations and leaves
others untouched.

## Channels

| Netty role | Bemo class |
| --- | --- |
| Server socket | `NativeServerSocketChannel` |
| TCP client or accepted socket | `NativeSocketChannel` |
| Unix domain socket | `NativeDomainSocketChannel` |

Use them with a Bemo event-loop group exactly as you would `EpollServerSocketChannel`
and friends. Writes are collected into bounded batches, and on Unix backends an
eligible write can complete during the call. See [I/O ownership](../transport-io.md)
for the precise send and receive rules.

## TLS

There are two independent ways to use Bemo's Rustls/AWS-LC TLS:

**Transport-owned TLS** runs below the pipeline. The channel feeds ciphertext to
Rustls and delivers plaintext to your handlers, and TLS records are retained
across partial writes until the socket accepts them:

```java
var serverTls = NativeTlsContext.server(transport, certificateChainPem, privateKeyPem, "h2", "http/1.1");
// In the child initializer:
((NativeSocketChannel) channel).tls(serverTls, null);

var clientTls = NativeTlsContext.client(transport, trustAnchorsPem, "h2");
bootstrap.channelFactory(() -> new NativeSocketChannel().tls(clientTls, "example.com"));
```

The `client` and `server` factories above bind the context to the default workload, so
they only serve channels in the default group. A context is bound to the workload its
channels register under, so a non-default group needs a context built for that workload:

```java
long workload = Workload.id(transport, Workload.Profile.SANDBOXED);
var clientTls = NativeTlsContext.client(transport, workload, trustAnchorsPem, "h2");
var serverTls = NativeTlsContext.server(transport, workload, certificateChainPem, privateKeyPem, "h2");
```

Using the wrong workload is rejected at session setup with an `IllegalArgumentException`
that names both the context's workload and the channel's.

**An `SSLEngine` adapter** plugs into Netty's `SslHandler`, for code that expects
an `SslContext`:

```java
SslContext context = NativeSslContextBuilder
    .forServer(transport, certificateChainPem, privateKeyPem)
    .applicationProtocols("h2", "http/1.1")
    .build();
pipeline.addLast(context.newHandler(channel.alloc()));
```

The builder also supports client identities, trust managers, protocol and cipher
suite selection, and STARTTLS. `NativeSslContextBuilder.supportedProtocols()`
and `supportedCipherSuites()` list what Rustls offers. TLS requires the
classpath; Netty's package-private ALPN interface cannot be reached from a named
module.

## Compression

`TransportNative.gzipNew(workload, level)` creates a reusable zlib-rs encoder,
and `gzipCompress` produces one independent gzip member as native storage
charged to the workload. Selecting a Bemo channel does not replace your
`HttpContentCompressor`; wire compression in explicitly. The
[framework examples](../framework-examples.md) show complete encoders for
Spring Boot, Micronaut, and Ktor.

## Buffer allocators

| Allocator | Memory | Use when |
| --- | --- | --- |
| Netty's default | JDK or Netty-managed | You need no native memory control |
| `NativeByteBufAllocator` | Native buffers charged to a transport owner, by handle | Allocations must count against a workload's budget and can be handed to native sends without copying |
| `NativeDirectByteBufAllocator` | Raw memory from Bemo's allocator (mimalloc, or the embedder's), unbudgeted | Netty direct buffers should come from the same allocator as the transport |

```java
bootstrap.childOption(ChannelOption.ALLOCATOR, new NativeDirectByteBufAllocator(transport));
```

`NativeDirectByteBufAllocator` needs a binding whose
`supportsNativeAllocation()` is true (both shipped bindings, from this
version). It is unpooled in Java because mimalloc already caches memory per
thread, and buffers may be released on any thread.
`nativeAllocatedBytes()` reports the process-wide live total for metrics and
leak checks. It relies on Netty 4.2's `CleanableDirectBuffer` hook.

## Shutdown

Close channels, shut down the event-loop group, then close the workload:

```java
server.close().sync();
group.shutdownGracefully().sync();
Workload.close(transport, Workload.DEFAULT);
```

Pending kernel operations keep their own buffer leases. A driver releases its
storage only after the kernel stops accessing it, so shutdown can wait on
in-flight io_uring or IOCP completions. The `dev.elide.TransportShutdown` JFR
event reports retries and any bytes still retained.
