# Stock Netty adapter

`bemo-netty` provides the extracted Netty 4.2 channels, event-loop handler,
reference-counted buffers, receive allocator, and Rustls-backed TLS integration.
It depends on `bemo-api` and stock Netty artifacts from Maven Central. Select
`bemo-ffm` for a regular JDK 22+ JVM or `bemo-native-image` for static C linking.

The Java package remains `dev.elide.bemo.transport` for migration compatibility.
`NativeRegion` stays in Elide because it implements Truffle interop. The
`io.netty.handler.ssl.ApplicationProtocolSslEngine` helper exposes Netty's
package-private ALPN accessor: use the classpath, not a named Netty JPMS module.

`tests/transport/java` exercises real TCP/Unix echo, native and SSLEngine TLS,
JSSE/OpenSSL interoperability, allocation ownership, shutdown, callback
reentrancy, and receive allocator behavior through both native bindings.
