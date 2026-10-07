package dev.elide.bemo.examples.spring;

import dev.elide.bemo.examples.BenchmarkTls;
import io.netty.channel.group.DefaultChannelGroup;
import io.netty.util.concurrent.GlobalEventExecutor;
import java.time.Duration;
import org.springframework.context.SmartLifecycle;
import org.springframework.http.client.ReactorResourceFactory;
import org.springframework.http.server.reactive.HttpHandler;
import org.springframework.http.server.reactive.ReactorHttpHandlerAdapter;
import reactor.netty.DisposableServer;
import reactor.netty.http.HttpProtocol;
import reactor.netty.http.server.HttpServer;

/** Serve the same Spring HTTP handler over HTTPS, sharing the HTTP listener's loops. */
final class HttpsListener implements SmartLifecycle {
  private final HttpHandler handler;
  private final ReactorResourceFactory resources;
  private final BenchmarkTls tls = new BenchmarkTls();
  private final DefaultChannelGroup connections =
      new DefaultChannelGroup(GlobalEventExecutor.INSTANCE);
  private volatile DisposableServer server;

  HttpsListener(HttpHandler handler, ReactorResourceFactory resources) {
    this.handler = handler;
    this.resources = resources;
  }

  @Override
  public void start() {
    if (server != null || !Boolean.parseBoolean(System.getProperty("bemo.tls.enabled", "true")))
      return;
    server =
        HttpServer.create()
            .host("127.0.0.1")
            .port(Integer.getInteger("bemo.tls.port", 8443))
            .protocol(HttpProtocol.HTTP11)
            .runOn(resources.getLoopResources())
            .secure(ssl -> ssl.sslContext(tls.context()))
            .doOnConnection(connection -> connections.add(connection.channel()))
            .handle(new ReactorHttpHandlerAdapter(handler))
            .bindNow(Duration.ofSeconds(30));
    System.out.println("Benchmark HTTPS listening on " + server.port());
  }

  @Override
  public void stop() {
    if (server != null) {
      server.disposeNow(Duration.ofSeconds(10));
      connections.close().awaitUninterruptibly();
      server = null;
    }
    tls.close();
  }

  @Override
  public boolean isRunning() {
    return server != null;
  }

  @Override
  public int getPhase() {
    return Integer.MAX_VALUE;
  }
}
