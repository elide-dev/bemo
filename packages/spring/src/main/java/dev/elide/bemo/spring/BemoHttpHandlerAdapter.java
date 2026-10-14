/*
 * Copyright (c) 2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.spring;

import dev.elide.bemo.transport.http.NativeHttpServer;
import java.net.URI;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicReference;
import org.jspecify.annotations.Nullable;
import org.reactivestreams.Publisher;
import org.springframework.core.io.buffer.DataBuffer;
import org.springframework.core.io.buffer.DataBufferUtils;
import org.springframework.core.io.buffer.DefaultDataBufferFactory;
import org.springframework.http.HttpCookie;
import org.springframework.http.HttpHeaders;
import org.springframework.http.HttpMethod;
import org.springframework.http.server.reactive.AbstractServerHttpRequest;
import org.springframework.http.server.reactive.AbstractServerHttpResponse;
import org.springframework.http.server.reactive.HttpHandler;
import org.springframework.http.server.reactive.SslInfo;
import org.springframework.util.LinkedMultiValueMap;
import org.springframework.util.MultiValueMap;
import reactor.adapter.JdkFlowAdapter;
import reactor.core.Disposable;
import reactor.core.publisher.Flux;
import reactor.core.publisher.Mono;

/**
 * Spring WebFlux request/response adapter over the native HTTP event loop, with no Netty
 * dependency.
 */
public final class BemoHttpHandlerAdapter implements NativeHttpServer.Handler {
  private final HttpHandler handler;
  private final String origin;

  public BemoHttpHandlerAdapter(HttpHandler handler, String origin) {
    URI uri = URI.create(origin);
    if (!List.of("http", "https").contains(uri.getScheme()) || uri.getRawAuthority() == null) {
      throw new IllegalArgumentException("HTTP origin required");
    }
    this.handler = handler;
    this.origin = origin;
  }

  @Override
  public CompletionStage<NativeHttpServer.Response> handle(NativeHttpServer.Request request) {
    CompletableFuture<NativeHttpServer.Response> ready = new CompletableFuture<>();
    AtomicReference<Disposable> application = new AtomicReference<>();
    Response response = new Response(ready);
    var unusedCancellationObserver =
        ready.whenComplete(
            (nativeResponse, failure) -> {
              if (failure != null) {
                Disposable task = application.get();
                if (task != null) task.dispose();
              } else {
                nativeResponse
                    .sent()
                    .whenComplete(
                        (ignored, writeFailure) -> {
                          if (writeFailure != null) {
                            Disposable task = application.get();
                            if (task != null) task.dispose();
                          }
                        });
              }
            });
    try {
      Request input = new Request(request, origin);
      Disposable task =
          handler
              .handle(input, response)
              .subscribe(
                  ignored -> {},
                  response::fail,
                  () -> response.setComplete().subscribe(ignored -> {}, response::fail));
      application.set(task);
      if (ready.isCompletedExceptionally()) task.dispose();
    } catch (Throwable failure) {
      response.fail(failure);
    }
    return ready;
  }

  private static final class Request extends AbstractServerHttpRequest {
    private final NativeHttpServer.Request nativeRequest;

    Request(NativeHttpServer.Request request, String origin) {
      super(HttpMethod.valueOf(request.method()), uri(request, origin), "", headers(request));
      nativeRequest = request;
    }

    private static HttpHeaders headers(NativeHttpServer.Request request) {
      HttpHeaders headers = new HttpHeaders();
      for (NativeHttpServer.Header header : request.headers())
        headers.add(header.name(), header.value());
      return headers;
    }

    private static URI uri(NativeHttpServer.Request request, String origin) {
      URI target = URI.create(request.target());
      if (target.isAbsolute()) return target;
      URI base = URI.create(origin);
      String authority = headers(request).getFirst(HttpHeaders.HOST);
      if (authority == null) authority = base.getRawAuthority();
      return URI.create(base.getScheme() + "://" + authority + request.target());
    }

    @Override
    public Flux<DataBuffer> getBody() {
      return JdkFlowAdapter.flowPublisherToFlux(nativeRequest.body())
          .map(bytes -> DefaultDataBufferFactory.sharedInstance.wrap(bytes));
    }

    @Override
    protected MultiValueMap<String, HttpCookie> initCookies() {
      MultiValueMap<String, HttpCookie> cookies = new LinkedMultiValueMap<>();
      List<String> fields = getHeaders().get(HttpHeaders.COOKIE);
      if (fields == null) return cookies;
      for (String field : fields) {
        for (String token : field.split(";", -1)) {
          int equals = token.indexOf('=');
          if (equals <= 0) continue;
          String name = token.substring(0, equals).trim();
          String value = token.substring(equals + 1).trim();
          if (value.length() >= 2 && value.startsWith("\"") && value.endsWith("\"")) {
            value = value.substring(1, value.length() - 1);
          }
          if (!name.isEmpty()) cookies.add(name, new HttpCookie(name, value));
        }
      }
      return cookies;
    }

    @Override
    public java.net.InetSocketAddress getLocalAddress() {
      return nativeRequest.localAddress();
    }

    @Override
    public java.net.InetSocketAddress getRemoteAddress() {
      return nativeRequest.remoteAddress();
    }

    @Override
    protected @Nullable SslInfo initSslInfo() {
      return null;
    }

    @Override
    @SuppressWarnings({
      "unchecked",
      "TypeParameterUnusedInFormals"
    }) // Spring native-access contract.
    public <T> T getNativeRequest() {
      return (T) nativeRequest;
    }
  }

  private static final class Response extends AbstractServerHttpResponse {
    private final CompletableFuture<NativeHttpServer.Response> ready;
    private NativeHttpServer.@Nullable Response nativeResponse;

    Response(CompletableFuture<NativeHttpServer.Response> ready) {
      super(DefaultDataBufferFactory.sharedInstance);
      this.ready = ready;
    }

    @Override
    @SuppressWarnings({
      "unchecked",
      "TypeParameterUnusedInFormals"
    }) // Spring native-access contract.
    public <T> T getNativeResponse() {
      return (T) this;
    }

    @Override
    protected void applyStatusCode() {}

    @Override
    protected void applyHeaders() {}

    @Override
    protected void applyCookies() {
      getCookies()
          .forEach(
              (name, cookies) ->
                  cookies.forEach(
                      cookie -> getHeaders().add(HttpHeaders.SET_COOKIE, cookie.toString())));
    }

    @Override
    protected Mono<Void> writeWithInternal(Publisher<? extends DataBuffer> body) {
      if (body instanceof Mono<?>) {
        return Mono.from(body)
            .map(buffer -> copyBody(buffer))
            .doOnDiscard(DataBuffer.class, DataBufferUtils::release)
            .defaultIfEmpty(new byte[0])
            .flatMap(bytes -> send(Flux.empty(), bytes));
      }
      return send(Flux.from(body));
    }

    @Override
    protected Mono<Void> writeAndFlushWithInternal(
        Publisher<? extends Publisher<? extends DataBuffer>> body) {
      // Every native part is flushed; concatMap preserves each inner publisher's order and demand.
      return send(Flux.from(body).concatMap(Flux::from, 1));
    }

    @Override
    public Mono<Void> setComplete() {
      return !isCommitted() ? doCommit(() -> send(Flux.empty(), new byte[0])) : Mono.empty();
    }

    private static byte[] copyBody(DataBuffer buffer) {
      try {
        byte[] bytes = new byte[buffer.readableByteCount()];
        buffer.read(bytes);
        return bytes;
      } finally {
        DataBufferUtils.release(buffer);
      }
    }

    private Mono<Void> send(Flux<? extends DataBuffer> body) {
      return send(body, null);
    }

    private Mono<Void> send(Flux<? extends DataBuffer> body, byte @Nullable [] completeBody) {
      return Mono.defer(
          () -> {
            List<NativeHttpServer.Header> headers = new ArrayList<>();
            getHeaders()
                .forEach(
                    (name, values) -> {
                      if (!name.equalsIgnoreCase(HttpHeaders.CONTENT_LENGTH)
                          && !name.equalsIgnoreCase(HttpHeaders.TRANSFER_ENCODING)) {
                        for (String value : values)
                          headers.add(new NativeHttpServer.Header(name, value));
                      }
                    });
            Flux<byte[]> bytes =
                body.map(buffer -> copyBody(buffer))
                    .doOnDiscard(DataBuffer.class, DataBufferUtils::release);
            long length = getHeaders().getContentLength();
            if (completeBody != null && length < 0) length = completeBody.length;
            // No body for the bodyless HTTP statuses; the native codec owns framing headers.
            int status = getStatusCode() == null ? 200 : getStatusCode().value();
            NativeHttpServer.Response response =
                new NativeHttpServer.Response(
                    status,
                    headers,
                    status == 204 || status == 304 ? 0 : length,
                    completeBody == null
                        ? JdkFlowAdapter.publisherToFlowPublisher(bytes)
                        : NativeHttpServer.fixedBody(completeBody));
            nativeResponse = response;
            if (!ready.complete(response)) {
              return Mono.error(new IllegalStateException("HTTP response already selected"));
            }
            return Mono.fromCompletionStage(response.sent());
          });
    }

    void fail(Throwable failure) {
      if (!ready.completeExceptionally(failure) && nativeResponse != null)
        nativeResponse.cancel(failure);
    }
  }
}
