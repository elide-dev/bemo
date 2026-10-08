package dev.elide.bemo.examples.micronaut;

import dev.elide.bemo.examples.BenchmarkTls;
import io.micronaut.context.annotation.Replaces;
import io.micronaut.http.server.netty.ssl.CertificateProvidedSslBuilder;
import io.micronaut.http.server.netty.ssl.ServerSslBuilder;
import io.micronaut.http.ssl.ServerSslConfiguration;
import io.netty.handler.ssl.SslContext;
import io.netty.util.ReferenceCountUtil;
import jakarta.annotation.PreDestroy;
import jakarta.inject.Singleton;
import java.util.Optional;

/** Supply Rustls or tcnative TLS at runtime; Micronaut keeps its HTTPS pipeline and routing. */
@Singleton
@Replaces(CertificateProvidedSslBuilder.class)
public final class BenchmarkSslBuilder implements ServerSslBuilder {
  private final ServerSslConfiguration configuration;
  private final BenchmarkTls tls = new BenchmarkTls();

  public BenchmarkSslBuilder(ServerSslConfiguration configuration) {
    this.configuration = configuration;
  }

  @Override
  public ServerSslConfiguration getSslConfiguration() {
    return configuration;
  }

  @Override
  public Optional<SslContext> build() {
    if (!configuration.isEnabled()) return Optional.empty();
    // Micronaut's context holder owns this reference; the bean retains its original.
    return Optional.of(ReferenceCountUtil.retain(tls.context()));
  }

  @PreDestroy
  void close() {
    tls.close();
  }
}
