package dev.elide.bemo.examples.micronaut;

import dev.elide.bemo.examples.BenchmarkCompression;
import dev.elide.bemo.examples.BenchmarkPayload;
import dev.elide.bemo.examples.BenchmarkTls;
import io.micronaut.core.annotation.NonBlocking;
import io.micronaut.http.HttpRequest;
import io.micronaut.http.HttpResponse;
import io.micronaut.http.HttpStatus;
import io.micronaut.http.MutableHttpResponse;
import io.micronaut.http.annotation.Controller;
import io.micronaut.http.annotation.Get;

@Controller
@NonBlocking
public final class PlaintextController {
  @Get(value = "/plaintext", produces = "text/plain")
  public String plaintext() {
    return "Hello, World!";
  }

  @Get(value = "/payload", produces = "text/plain")
  public byte[] payload() {
    return BenchmarkPayload.body();
  }

  @Get(value = "/compression", produces = "text/plain")
  public MutableHttpResponse<byte[]> compression(HttpRequest<?> request) {
    return largeResponse(request, true, false);
  }

  @Get(value = "/tls", produces = "text/plain")
  public MutableHttpResponse<byte[]> tls(HttpRequest<?> request) {
    return largeResponse(request, false, true);
  }

  @Get(value = "/tls-compression", produces = "text/plain")
  public MutableHttpResponse<byte[]> tlsCompression(HttpRequest<?> request) {
    return largeResponse(request, true, true);
  }

  private static MutableHttpResponse<byte[]> largeResponse(
      HttpRequest<?> request, boolean compress, boolean tls) {
    if (tls && !request.isSecure()) return HttpResponse.<byte[]>status(HttpStatus.UPGRADE_REQUIRED);
    boolean gzip =
        compress && BenchmarkPayload.acceptsGzip(request.getHeaders().get("Accept-Encoding"));
    byte[] body = gzip ? BenchmarkCompression.gzip() : BenchmarkPayload.body();
    MutableHttpResponse<byte[]> response = HttpResponse.ok(body).contentLength(body.length);
    if (compress) response.header("Vary", "Accept-Encoding");
    if (gzip)
      response
          .header("Content-Encoding", "gzip")
          .header("X-Compression-Provider", BenchmarkCompression.provider());
    if (tls) response.header("X-TLS-Provider", BenchmarkTls.provider());
    return response;
  }
}
