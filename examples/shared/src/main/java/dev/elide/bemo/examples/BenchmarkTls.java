package dev.elide.bemo.examples;

import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.tls.NativeSslContextBuilder;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.util.ReferenceCountUtil;
import java.io.ByteArrayInputStream;
import java.io.IOException;

/** Public localhost test identity; the listener owns this context and its workload. */
public final class BenchmarkTls implements AutoCloseable {
  private TransportNative api;
  private long owner;
  private SslContext context;

  public synchronized SslContext context() {
    if (context != null) return context;
    try {
      byte[] certificate = resource("localhost-cert.pem");
      byte[] key = resource("localhost-key.pem");
      if (BenchmarkPayload.bemoEnabled()) {
        api = BemoRuntime.binding();
        owner = api.ownerNew(16 * 1024 * 1024);
        if (owner == 0) throw new IllegalStateException("TLS workload admission failed");
        context =
            NativeSslContextBuilder.forServer(api, certificate, key)
                .workload(owner)
                .applicationProtocols("http/1.1")
                .build();
        System.out.println("Bemo TLS enabled (Rustls/aws-lc-rs)");
      } else {
        context =
            SslContextBuilder.forServer(
                    new ByteArrayInputStream(certificate), new ByteArrayInputStream(key))
                .sslProvider(SslProvider.JDK)
                .protocols("TLSv1.2", "TLSv1.3")
                .build();
      }
      return context;
    } catch (IOException | RuntimeException | Error failure) {
      close();
      throw new IllegalStateException("Cannot configure benchmark TLS", failure);
    }
  }

  public static String provider() {
    return BenchmarkPayload.bemoEnabled() ? "bemo-rustls-aws-lc" : "jdk";
  }

  private static byte[] resource(String name) throws IOException {
    try (var input = BenchmarkTls.class.getResourceAsStream("/benchmark-tls/" + name)) {
      if (input == null) throw new IOException("Missing benchmark TLS fixture " + name);
      return input.readAllBytes();
    }
  }

  @Override
  public synchronized void close() {
    if (context != null) {
      ReferenceCountUtil.release(context);
      context = null;
    }
    if (owner != 0) {
      if (api.ownerUsed(owner) != 0)
        throw new IllegalStateException("TLS buffers were not reclaimed");
      if (api.ownerRelease(owner) != 0)
        throw new IllegalStateException("TLS workload release failed");
      owner = 0;
      System.out.println("Bemo TLS workload reclaimed");
    }
  }
}
