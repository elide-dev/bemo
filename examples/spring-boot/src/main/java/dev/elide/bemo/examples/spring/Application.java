package dev.elide.bemo.examples.spring;

import dev.elide.bemo.examples.BenchmarkCompression;
import dev.elide.bemo.examples.BenchmarkPayload;
import dev.elide.bemo.examples.BenchmarkTls;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.context.annotation.Bean;
import org.springframework.core.env.Environment;
import org.springframework.http.ResponseEntity;
import org.springframework.http.client.ReactorResourceFactory;
import org.springframework.http.server.reactive.HttpHandler;
import org.springframework.http.server.reactive.ServerHttpRequest;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RestController;
import reactor.netty.resources.LoopResources;

@SpringBootApplication
@RestController
public class Application {
  public static void main(String[] args) {
    SpringApplication.run(Application.class, args);
  }

  @GetMapping(value = "/plaintext", produces = "text/plain")
  public String plaintext() {
    return "Hello, World!";
  }

  @GetMapping(value = "/payload", produces = "text/plain")
  public byte[] payload() {
    return BenchmarkPayload.body();
  }

  @GetMapping(value = "/compression", produces = "text/plain")
  public ResponseEntity<byte[]> compression(
      @RequestHeader(value = "Accept-Encoding", required = false) String acceptEncoding) {
    return largeResponse(acceptEncoding, true, false);
  }

  @GetMapping(value = "/tls", produces = "text/plain")
  public ResponseEntity<byte[]> tls(ServerHttpRequest request) {
    if (!"https".equals(request.getURI().getScheme())) return ResponseEntity.status(426).build();
    return largeResponse(null, false, true);
  }

  @GetMapping(value = "/tls-compression", produces = "text/plain")
  public ResponseEntity<byte[]> tlsCompression(
      ServerHttpRequest request,
      @RequestHeader(value = "Accept-Encoding", required = false) String acceptEncoding) {
    if (!"https".equals(request.getURI().getScheme())) return ResponseEntity.status(426).build();
    return largeResponse(acceptEncoding, true, true);
  }

  private static ResponseEntity<byte[]> largeResponse(
      String acceptEncoding, boolean compress, boolean tls) {
    boolean gzip = compress && BenchmarkPayload.acceptsGzip(acceptEncoding);
    byte[] body = gzip ? BenchmarkCompression.gzip() : BenchmarkPayload.body();
    var response = ResponseEntity.ok();
    if (compress) response.header("Vary", "Accept-Encoding");
    if (gzip)
      response
          .header("Content-Encoding", "gzip")
          .header("X-Compression-Provider", BenchmarkCompression.provider());
    if (tls) response.header("X-TLS-Provider", BenchmarkTls.provider());
    return response.contentLength(body.length).body(body);
  }

  @Bean(destroyMethod = "stop")
  HttpsListener httpsListener(HttpHandler handler, ReactorResourceFactory resources) {
    return new HttpsListener(handler, resources);
  }

  @Bean(destroyMethod = "dispose")
  BemoLoopResources bemoLoops() {
    return new BemoLoopResources(Integer.getInteger("bemo.threads", 2));
  }

  @Bean
  ReactorResourceFactory reactorResources(BemoLoopResources loops, Environment environment) {
    // AOT freezes bean conditions. Select resources at runtime for both transport modes.
    ReactorResourceFactory resources = new ReactorResourceFactory();
    resources.setUseGlobalResources(false);
    resources.setLoopResourcesSupplier(
        () ->
            environment.getProperty("bemo.enabled", Boolean.class, true)
                ? loops
                : LoopResources.create("netty", 2, true));
    return resources;
  }
}
