/*
 * Copyright (c) 2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.http.NativeHttpServer;
import java.io.ByteArrayOutputStream;
import java.net.Socket;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Flow;
import java.util.concurrent.TimeUnit;

/** The same lifecycle/streaming contract runs through FFM and Bali's direct image backend. */
public final class NativeHttpServerTest {
  private static void require(boolean ok, String operation) {
    if (!ok) throw new AssertionError(operation);
  }

  private static Flow.Publisher<byte[]> body(List<byte[]> chunks) {
    return subscriber ->
        subscriber.onSubscribe(
            new Flow.Subscription() {
              int next;
              boolean ended;

              @Override
              public void request(long n) {
                require(n == 1, "one response item after physical write credit");
                if (ended) return;
                if (next == chunks.size()) {
                  ended = true;
                  subscriber.onComplete();
                } else subscriber.onNext(chunks.get(next++));
              }

              @Override
              public void cancel() {
                ended = true;
              }
            });
  }

  private static CompletableFuture<NativeHttpServer.Response> echo(
      NativeHttpServer.Request request, boolean complete) {
    CompletableFuture<NativeHttpServer.Response> result = new CompletableFuture<>();
    request
        .body()
        .subscribe(
            new Flow.Subscriber<>() {
              Flow.Subscription subscription;
              final ByteArrayOutputStream input = new ByteArrayOutputStream();

              @Override
              public void onSubscribe(Flow.Subscription next) {
                subscription = next;
                next.request(1);
              }

              @Override
              public void onNext(byte[] bytes) {
                input.writeBytes(bytes);
                subscription.request(1);
              }

              @Override
              public void onError(Throwable failure) {
                result.completeExceptionally(failure);
              }

              @Override
              public void onComplete() {
                byte[] prefix =
                    (request.method() + " " + request.target() + " ")
                        .getBytes(StandardCharsets.US_ASCII);
                byte[] payload = input.toByteArray();
                if (request.target().startsWith("/headers")) {
                  String values =
                      request.headers().stream()
                          .filter(header -> header.name().equalsIgnoreCase("x-probe"))
                          .map(NativeHttpServer.Header::value)
                          .collect(java.util.stream.Collectors.joining("|"));
                  payload =
                      (request.version() + " " + values).getBytes(StandardCharsets.ISO_8859_1);
                }
                result.complete(
                    new NativeHttpServer.Response(
                        200,
                        List.of(
                            new NativeHttpServer.Header(
                                "content-type", "application/octet-stream")),
                        request.target().contains("chunked") ? -1 : prefix.length + payload.length,
                        complete
                            ? NativeHttpServer.fixedBody(concat(prefix, payload))
                            : body(List.of(prefix, payload))));
              }
            });
    return result;
  }

  private static byte[] concat(byte[] prefix, byte[] payload) {
    byte[] result = Arrays.copyOf(prefix, prefix.length + payload.length);
    System.arraycopy(payload, 0, result, prefix.length, payload.length);
    return result;
  }

  public static void verify(TransportNative api, Path certificate, Path key) throws Exception {
    verify(api, certificate, key, 1);
  }

  public static void verify(TransportNative api, Path certificate, Path key, int workers)
      throws Exception {
    verifyMode(api, certificate, key, workers, false);
    verifyMode(api, certificate, key, workers, true);
  }

  private static void verifyMode(
      TransportNative api, Path certificate, Path key, int workers, boolean complete)
      throws Exception {
    NativeHttpServer unstarted =
        new NativeHttpServer(
            api,
            "127.0.0.1",
            0,
            1024 * 1024,
            1024,
            16,
            request -> echo(request, complete),
            null,
            null,
            workers);
    unstarted.close();
    unstarted.stopped().toCompletableFuture().get(1, TimeUnit.SECONDS);
    require(
        unstarted.start().toCompletableFuture().isCompletedExceptionally(),
        "closed server cannot restart");
    for (boolean tls : new boolean[] {false, true}) {
      byte[] cert = tls ? Files.readAllBytes(certificate) : null;
      byte[] privateKey = tls ? Files.readAllBytes(key) : null;
      NativeHttpServer server =
          new NativeHttpServer(
              api,
              "127.0.0.1",
              0,
              16 * 1024 * 1024,
              2 * 1024 * 1024,
              1024,
              request -> echo(request, complete),
              cert,
              privateKey,
              workers);
      int port = server.start().toCompletableFuture().get(5, TimeUnit.SECONDS);
      HttpClient.Builder builder = HttpClient.newBuilder().version(HttpClient.Version.HTTP_1_1);
      if (tls) {
        java.security.KeyStore store = java.security.KeyStore.getInstance("PKCS12");
        store.load(null, null);
        try (var input = Files.newInputStream(certificate)) {
          store.setCertificateEntry(
              "fixture",
              java.security.cert.CertificateFactory.getInstance("X.509")
                  .generateCertificate(input));
        }
        var trust =
            javax.net.ssl.TrustManagerFactory.getInstance(
                javax.net.ssl.TrustManagerFactory.getDefaultAlgorithm());
        trust.init(store);
        var ssl = javax.net.ssl.SSLContext.getInstance("TLS");
        ssl.init(null, trust.getTrustManagers(), null);
        builder.sslContext(ssl);
      }
      try (HttpClient client = builder.build()) {
        String base = (tls ? "https" : "http") + "://127.0.0.1:" + port;
        var first =
            client.send(
                HttpRequest.newBuilder(URI.create(base + "/ping?value=1")).build(),
                HttpResponse.BodyHandlers.ofString());
        require(
            first.statusCode() == 200 && first.body().equals("GET /ping?value=1 "),
            "empty request and exact target");
        var headers =
            client.send(
                HttpRequest.newBuilder(URI.create(base + "/headers?escaped=%2F"))
                    .header("X-Probe", "alpha")
                    .header("X-Probe", "beta")
                    .build(),
                HttpResponse.BodyHandlers.ofString());
        require(
            headers.body().equals("GET /headers?escaped=%2F HTTP/1.1 alpha|beta"),
            "duplicate header order, exact escaped target and HTTP version");
        var head =
            client.send(
                HttpRequest.newBuilder(URI.create(base + "/head"))
                    .method("HEAD", HttpRequest.BodyPublishers.noBody())
                    .build(),
                HttpResponse.BodyHandlers.ofByteArray());
        require(
            head.statusCode() == 200
                && head.body().length == 0
                && head.headers().firstValueAsLong("content-length").orElse(-1)
                    == "HEAD /head ".length(),
            "HEAD metadata without body");
        byte[] bytes = new byte[1024 * 1024];
        for (int i = 0; i < bytes.length; i++) bytes[i] = (byte) (i * 31);
        var fixed =
            client.send(
                HttpRequest.newBuilder(URI.create(base + "/large"))
                    .POST(HttpRequest.BodyPublishers.ofByteArray(bytes))
                    .build(),
                HttpResponse.BodyHandlers.ofByteArray());
        byte[] prefix = "POST /large ".getBytes(StandardCharsets.US_ASCII);
        require(
            fixed.statusCode() == 200 && fixed.body().length == prefix.length + bytes.length,
            "large response length");
        require(
            Arrays.equals(
                Arrays.copyOfRange(fixed.body(), prefix.length, fixed.body().length), bytes),
            "large body split at native response window");
        var chunked =
            client.send(
                HttpRequest.newBuilder(URI.create(base + "/chunked"))
                    .POST(
                        HttpRequest.BodyPublishers.ofInputStream(
                            () -> new java.io.ByteArrayInputStream(bytes)))
                    .build(),
                HttpResponse.BodyHandlers.ofByteArray());
        prefix = "POST /chunked ".getBytes(StandardCharsets.US_ASCII);
        require(
            chunked.statusCode() == 200 && chunked.body().length == prefix.length + bytes.length,
            "chunked response length");
        require(
            Arrays.equals(
                Arrays.copyOfRange(chunked.body(), prefix.length, chunked.body().length), bytes),
            "chunked request/response data");
        if (!tls) {
          try (Socket latin = new Socket("127.0.0.1", port)) {
            latin.setSoTimeout(5000);
            latin
                .getOutputStream()
                .write(
                    "GET /headers?latin=1 HTTP/1.0\r\nX-Probe: caf\u00e9\r\nx-probe:\r\n\r\n"
                        .getBytes(StandardCharsets.ISO_8859_1));
            String wire =
                new String(latin.getInputStream().readAllBytes(), StandardCharsets.ISO_8859_1);
            require(
                wire.endsWith("GET /headers?latin=1 HTTP/1.0 caf\u00e9|"),
                "HTTP/1.0, Latin-1 and empty duplicate header values");
          }
          try (Socket malformed = new Socket("127.0.0.1", port)) {
            malformed.setSoTimeout(5000);
            malformed
                .getOutputStream()
                .write(
                    "GET / HTTP/1.1\r\nHost: a\r\nBad Header: value\r\n\r\n"
                        .getBytes(StandardCharsets.US_ASCII));
            String status =
                new java.io.BufferedReader(
                        new java.io.InputStreamReader(
                            malformed.getInputStream(), StandardCharsets.US_ASCII))
                    .readLine();
            require(
                status == null || status.startsWith("HTTP/1.1 400"),
                "malformed request rejected: " + status);
          }
          var after =
              client.send(
                  HttpRequest.newBuilder(URI.create(base + "/after")).build(),
                  HttpResponse.BodyHandlers.ofString());
          require(after.body().equals("GET /after "), "malformed peer leaves server healthy");
        }
      } finally {
        server.close();
        server.stopped().toCompletableFuture().get(1, TimeUnit.SECONDS);
      }
    }
    System.out.println(
        "Native HTTP framework loop: streaming, TLS, malformed input and retirement passed");
  }

  public static void main(String[] args) throws Exception {
    verify(new FfmTransportNative(Path.of(args[0])), Path.of(args[1]), Path.of(args[2]));
  }
}
