/*
 * Copyright (c) 2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport.http;

import dev.elide.bemo.transport.TransportNative;
import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.Executor;
import java.util.concurrent.Flow;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import org.jspecify.annotations.Nullable;

/**
 * HTTP/1.1 event loops shared by framework adapters. Native driver operations stay on each worker.
 * Request bodies retain native receive-window credit until requested; response publishers receive
 * one item of demand at a time, after the previous part reaches the wire. No framework or image
 * classes cross this API.
 */
public final class NativeHttpServer implements AutoCloseable {
  public record Header(String name, String value) {}

  public record Request(
      String method,
      String target,
      List<Header> headers,
      Flow.Publisher<byte[]> body,
      InetSocketAddress localAddress,
      InetSocketAddress remoteAddress,
      boolean secure,
      String version) {
    /** Queue application work on this connection's owner; rejects after server shutdown. */
    public Executor executor() {
      if (body instanceof Inbound inbound) return inbound.executor();
      throw new IllegalStateException("Request has no native worker");
    }
  }

  /** A known body. The caller must not modify bytes until the response completes. */
  public static Flow.Publisher<byte[]> fixedBody(byte[] bytes) {
    return new FixedBody(java.util.Objects.requireNonNull(bytes));
  }

  private record FixedBody(byte[] bytes) implements Flow.Publisher<byte[]> {
    @Override
    public void subscribe(Flow.Subscriber<? super byte[]> subscriber) {
      subscriber.onSubscribe(
          new Flow.Subscription() {
            private boolean done;
            private boolean cancelled;

            @Override
            public void request(long n) {
              if (done) return;
              done = true;
              if (n <= 0) {
                subscriber.onError(new IllegalArgumentException("positive demand required"));
                return;
              }
              if (bytes.length != 0) subscriber.onNext(bytes);
              if (!cancelled) subscriber.onComplete();
            }

            @Override
            public void cancel() {
              cancelled = true;
              done = true;
            }
          });
    }
  }

  /** A response's completion represents physical transmission, or disconnect/cancellation. */
  public static final class Response {
    public final int status;
    public final List<Header> headers;
    public final long length;
    public final Flow.Publisher<byte[]> body;
    private final CompletableFuture<Void> sent = new CompletableFuture<>();

    public Response(int status, List<Header> headers, long length, Flow.Publisher<byte[]> body) {
      if (status < 200 || status > 599 || length < -1)
        throw new IllegalArgumentException("response");
      this.status = status;
      this.headers = List.copyOf(headers);
      this.length = length;
      this.body = body;
    }

    public CompletionStage<Void> sent() {
      return sent.minimalCompletionStage();
    }

    /** Abort an application response after its headers have been selected. */
    public void cancel(Throwable failure) {
      sent.completeExceptionally(failure);
    }
  }

  @FunctionalInterface
  public interface Handler {
    CompletionStage<Response> handle(Request request);
  }

  /** One shared native workload; drivers and established connections remain worker-confined. */
  private static final class WorkerGroup implements AutoCloseable {
    private final NativeHttpServer parent;
    private final List<NativeHttpServer> workers = new ArrayList<>();
    private final Set<Accepted> pending = new HashSet<>();
    private final AtomicInteger remaining;
    private final AtomicInteger exchanges = new AtomicInteger();
    private final AtomicReference<Throwable> failure = new AtomicReference<>();
    private volatile boolean closing;
    private boolean started;
    private int next;
    private long owner;

    private static final class Accepted {
      final AtomicLong socket;

      Accepted(long socket) {
        this.socket = new AtomicLong(socket);
      }

      void discard(TransportNative api) {
        long handle = socket.getAndSet(0);
        if (handle != 0) api.socketDiscard(handle);
      }
    }

    WorkerGroup(NativeHttpServer parent, int count) {
      this.parent = parent;
      remaining = new AtomicInteger(count);
      for (int i = 0; i < count; i++) {
        NativeHttpServer worker =
            new NativeHttpServer(
                parent.api,
                parent.host,
                parent.port,
                parent.memoryLimit,
                parent.bodyLimit,
                parent.exchangeLimit,
                parent.handler,
                parent.certificate,
                parent.privateKey,
                1,
                this,
                i);
        workers.add(worker);
        worker.stopped.whenComplete((ignored, error) -> workerStopped(error));
      }
    }

    synchronized CompletionStage<Integer> start() {
      if (started || closing) return parent.bound;
      started = true;
      try {
        owner = parent.api.ownerNew(parent.memoryLimit);
        require(owner != 0, "shared owner admission");
        CompletableFuture.allOf(
                workers.stream().map(worker -> worker.bound).toArray(CompletableFuture<?>[]::new))
            .whenComplete(
                (ignored, error) -> {
                  if (!closing) {
                    if (error != null) fail(error);
                    else parent.bound.complete(workers.get(0).bound.join());
                  }
                });
        for (NativeHttpServer worker : workers) worker.start();
      } catch (Throwable error) {
        fail(error);
      }
      return parent.bound;
    }

    private void fail(Throwable error) {
      failure.compareAndSet(null, error);
      parent.bound.completeExceptionally(error);
      stopAll();
    }

    private void stopAll() {
      List<Accepted> abandoned;
      synchronized (this) {
        if (closing) return;
        closing = true;
        abandoned = List.copyOf(pending);
        pending.clear();
      }
      for (Accepted socket : abandoned) {
        try {
          socket.discard(parent.api);
        } catch (Throwable error) {
          failure.compareAndSet(null, error);
        }
      }
      parent.bound.completeExceptionally(
          new java.util.concurrent.CancellationException("HTTP worker group stopped"));
      for (NativeHttpServer worker : workers) {
        try {
          worker.stopLoop();
        } catch (Throwable error) {
          failure.compareAndSet(null, error);
        }
      }
    }

    private void workerStopped(@Nullable Throwable error) {
      if (error != null) fail(error);
      else if (!closing) fail(new IllegalStateException("HTTP worker stopped unexpectedly"));
      if (remaining.decrementAndGet() != 0) return;
      try {
        require(exchanges.get() == 0, "shared exchange reclamation");
        if (owner != 0) {
          require(parent.api.ownerUsed(owner) == 0, "shared native storage reclamation");
          require(parent.api.ownerRelease(owner) == 0, "shared owner retirement");
        }
      } catch (Throwable cleanup) {
        failure.compareAndSet(null, cleanup);
      }
      Throwable result = failure.get();
      if (result == null) parent.stopped.complete(null);
      else parent.stopped.completeExceptionally(result);
    }

    synchronized void accept(long socket) {
      if (closing) {
        parent.api.socketDiscard(socket);
        return;
      }
      NativeHttpServer worker = workers.get(next);
      next = (next + 1) % workers.size();
      Accepted lease = new Accepted(socket);
      pending.add(lease);
      if (!worker.submit(
          () -> {
            long handle = lease.socket.getAndSet(0);
            synchronized (this) {
              pending.remove(lease);
            }
            if (handle != 0) {
              if (worker.running.get()) worker.adopt(handle);
              else parent.api.socketDiscard(handle);
            }
          })) {
        pending.remove(lease);
        lease.discard(parent.api);
      }
    }

    boolean admitExchange() {
      for (; ; ) {
        int current = exchanges.get();
        if (current >= parent.exchangeLimit) return false;
        if (exchanges.compareAndSet(current, current + 1)) return true;
      }
    }

    @Override
    public void close() {
      stopAll();
      for (NativeHttpServer worker : workers) worker.awaitLoop();
    }
  }

  private static final int BATCH_SIZE = 256;
  private static final int MAX_RESPONSE_PART = 64 * 1024;
  private final @Nullable WorkerGroup workerGroup;
  private final @Nullable WorkerGroup shared;
  private final int workerIndex;
  private final TransportNative api;
  private final String host;
  private final int port;
  private final Handler handler;
  private final long memoryLimit;
  private final long bodyLimit;
  private final int exchangeLimit;
  private final byte @Nullable [] certificate;
  private final byte @Nullable [] privateKey;
  private final ConcurrentLinkedQueue<Runnable> tasks = new ConcurrentLinkedQueue<>();
  private final AtomicBoolean running = new AtomicBoolean();

  private static final class ApplicationTask implements Runnable {
    private final Runnable action;

    ApplicationTask(Runnable action) {
      this.action = action;
    }

    @Override
    public void run() {
      action.run();
    }
  }

  private final Executor executor =
      task -> {
        if (!submit(new ApplicationTask(java.util.Objects.requireNonNull(task))))
          throw new RejectedExecutionException("Native HTTP worker stopped");
      };
  private boolean closed;
  private final AtomicLong wakeDriver = new AtomicLong();
  private final CompletableFuture<Integer> bound = new CompletableFuture<>();
  private final CompletableFuture<Void> stopped = new CompletableFuture<>();
  private final Map<Long, Exchange> exchanges = new HashMap<>();
  private final Set<Long> sockets = new HashSet<>();

  private record Peer(InetSocketAddress local, InetSocketAddress remote) {}

  private final Map<Long, Peer> peers = new HashMap<>();
  private long owner;
  private long driver;
  private long listener;
  private long tls;
  private long batch;
  private long descriptorBuffer;
  private long descriptor;
  private @Nullable Thread thread;

  public NativeHttpServer(
      TransportNative api,
      String host,
      int port,
      long memoryLimit,
      long bodyLimit,
      int exchangeLimit,
      Handler handler) {
    this(api, host, port, memoryLimit, bodyLimit, exchangeLimit, handler, null, null);
  }

  public NativeHttpServer(
      TransportNative api,
      String host,
      int port,
      long memoryLimit,
      long bodyLimit,
      int exchangeLimit,
      Handler handler,
      byte @Nullable [] certificate,
      byte @Nullable [] privateKey) {
    this(
        api,
        host,
        port,
        memoryLimit,
        bodyLimit,
        exchangeLimit,
        handler,
        certificate,
        privateKey,
        1);
  }

  /** Workers share admission limits; accepted connections stay on their selected worker. */
  public NativeHttpServer(
      TransportNative api,
      String host,
      int port,
      long memoryLimit,
      long bodyLimit,
      int exchangeLimit,
      Handler handler,
      byte @Nullable [] certificate,
      byte @Nullable [] privateKey,
      int workers) {
    this(
        api,
        host,
        port,
        memoryLimit,
        bodyLimit,
        exchangeLimit,
        handler,
        certificate,
        privateKey,
        workers,
        null,
        0);
  }

  private NativeHttpServer(
      TransportNative api,
      String host,
      int port,
      long memoryLimit,
      long bodyLimit,
      int exchangeLimit,
      Handler handler,
      byte @Nullable [] certificate,
      byte @Nullable [] privateKey,
      int workers,
      @Nullable WorkerGroup shared,
      int workerIndex) {
    if (port < 0
        || port > 65535
        || memoryLimit < 16384
        || bodyLimit < 0
        || exchangeLimit < 1
        || workers < 1
        || (certificate == null) != (privateKey == null))
      throw new IllegalArgumentException("server limits");
    this.shared = shared;
    this.workerIndex = workerIndex;
    this.api = api;
    this.host = host;
    this.port = port;
    this.memoryLimit = memoryLimit;
    this.bodyLimit = bodyLimit;
    this.exchangeLimit = exchangeLimit;
    this.handler = handler;
    this.certificate = certificate == null ? null : certificate.clone();
    this.privateKey = privateKey == null ? null : privateKey.clone();
    this.workerGroup = workers == 1 ? null : new WorkerGroup(this, workers);
  }

  /** Start once; the result contains the actual bound port. */
  public synchronized CompletionStage<Integer> start() {
    if (closed || stopped.isDone())
      return CompletableFuture.failedFuture(new IllegalStateException("HTTP server closed"));
    if (workerGroup != null) return workerGroup.start();
    if (thread == null) {
      running.set(true);
      thread = new Thread(this::run, shared == null ? "bemo-http" : "bemo-http-" + workerIndex);
      try {
        thread.start();
      } catch (RuntimeException | Error failure) {
        running.set(false);
        bound.completeExceptionally(failure);
        stopped.completeExceptionally(failure);
        throw failure;
      }
    }
    return bound;
  }

  public CompletionStage<Void> stopped() {
    return stopped;
  }

  private boolean submit(Runnable task) {
    if (!running.get()) return false;
    tasks.add(task);
    if (!running.get() && tasks.remove(task)) return false;
    long handle = wakeDriver.get();
    if (handle != 0 && Thread.currentThread() != thread) api.driverWake(handle);
    return true;
  }

  private static void require(boolean ok, String operation) {
    if (!ok) throw new IllegalStateException("Bemo HTTP: " + operation);
  }

  private long allocate(int capacity) {
    long handle = api.bufferNew(owner, Math.max(1, capacity));
    require(handle != 0, "native storage admission");
    return handle;
  }

  private long upload(byte[] bytes) {
    long handle = allocate(bytes.length);
    try {
      api.writeBytes(api.bufferAddress(handle), bytes, 0, bytes.length);
      require(api.bufferFreeze(handle, bytes.length) == 0, "input freeze");
      return handle;
    } catch (RuntimeException | Error failure) {
      api.bufferRelease(handle);
      throw failure;
    }
  }

  private void initialize() throws Exception {
    owner = shared == null ? api.ownerNew(memoryLimit) : shared.owner;
    require(owner != 0, "owner admission");
    driver = api.driverNew(owner, 0, BATCH_SIZE);
    require(driver != 0, "driver initialization");
    wakeDriver.set(driver);
    batch = allocate(40 * BATCH_SIZE);
    descriptorBuffer = allocate(32);
    descriptor = api.bufferAddress(descriptorBuffer);
    if (certificate != null && privateKey != null) {
      long cert = upload(certificate);
      long key = 0;
      long alpn = 0;
      try {
        key = upload(privateKey);
        alpn = upload("\010http/1.1".getBytes(StandardCharsets.US_ASCII));
        tls = api.tlsServer(owner, cert, key, alpn);
        require(tls != 0, "TLS context");
      } finally {
        api.bufferRelease(cert);
        if (key != 0) api.bufferRelease(key);
        if (alpn != 0) api.bufferRelease(alpn);
      }
    }
    if (shared != null && workerIndex != 0) {
      bound.complete(0);
      return;
    }
    byte[] address = InetAddress.getByName(host).getAddress();
    require(address.length == 4, "IPv4 listen address");
    long endpoint = allocate(24);
    try {
      api.bufferView(endpoint)
          .order(ByteOrder.nativeOrder())
          .put(address)
          .putShort(16, (short) port)
          .putShort(18, (short) 4);
      require(api.bufferFreeze(endpoint, 24) == 0, "endpoint");
      listener = api.socketListen(owner, driver, endpoint, 256, 1);
      require(listener != 0, "listen");
    } finally {
      api.bufferRelease(endpoint);
    }
    long boundAddress = allocate(24);
    try {
      require(api.socketAddress(driver, listener, 0, boundAddress) == 0, "bound address");
      int actualPort =
          Short.toUnsignedInt(
              api.bufferView(boundAddress).order(ByteOrder.nativeOrder()).getShort(16));
      require(api.socketAccept(owner, driver, listener) != 0, "accept submission");
      bound.complete(actualPort);
    } finally {
      api.bufferRelease(boundAddress);
    }
  }

  private void run() {
    Throwable failure = null;
    try {
      initialize();
      ByteBuffer events = api.bufferView(batch).order(ByteOrder.nativeOrder());
      boolean progressed = false;
      while (running.get()) {
        Runnable task;
        while ((task = tasks.poll()) != null) {
          task.run();
          progressed = true;
        }
        // Flush work queued by Java before entering the blocking native poll.
        int count = api.driverPoll(driver, progressed ? 0 : 10_000_000L, batch, BATCH_SIZE);
        progressed = count > 0;
        require(count >= 0, "poll");
        for (int i = 0; i < count; i++) {
          int offset = i * 40;
          event(
              events.getLong(offset),
              events.getLong(offset + 8),
              events.getLong(offset + 16),
              events.getLong(offset + 24),
              events.getInt(offset + 32));
        }
      }
    } catch (Throwable error) {
      failure = error;
      bound.completeExceptionally(error);
    } finally {
      running.set(false);
      wakeDriver.set(0);
      try {
        retire();
      } catch (Throwable error) {
        if (failure == null) failure = error;
        else failure.addSuppressed(error);
      }
      // Accepted continuations must run even if shutdown overtook the normal task phase.
      // Late producers observe running=false and remove/reject their own queued task.
      Runnable queued;
      while ((queued = tasks.poll()) != null) {
        if (queued instanceof ApplicationTask) {
          try {
            queued.run();
          } catch (Throwable error) {
            if (failure == null) failure = error;
            else failure.addSuppressed(error);
          }
        }
      }
      if (failure == null) stopped.complete(null);
      else stopped.completeExceptionally(failure);
    }
  }

  private record RequestHead(String method, String target, List<Header> headers, String version) {}

  /** Snapshot the ABI's head and span table before invoking application code. */
  private RequestHead requestHead(long exchange) {
    ByteBuffer layout = ByteBuffer.wrap(api.bytes(exchange, 32)).order(ByteOrder.nativeOrder());
    int length = Math.toIntExact(layout.getLong(8));
    int slots = layout.getInt(24);
    require(length >= 0 && slots >= 4 && (slots - 4) % 4 == 0, "request head layout");
    byte[] head = api.bytes(layout.getLong(0), length);
    ByteBuffer spans =
        ByteBuffer.wrap(api.bytes(layout.getLong(16), Math.multiplyExact(slots, 4)))
            .order(ByteOrder.nativeOrder());
    List<Header> headers = new ArrayList<>((slots - 4) / 4);
    String method = headField(head, spans, 0);
    String target = headField(head, spans, 2);
    for (int slot = 4; slot < slots; slot += 4) {
      headers.add(new Header(headField(head, spans, slot), headField(head, spans, slot + 2)));
    }
    return new RequestHead(
        method, target, List.copyOf(headers), layout.get(29) == 0 ? "HTTP/1.0" : "HTTP/1.1");
  }

  private static String headField(byte[] head, ByteBuffer spans, int slot) {
    int start = spans.getInt(slot * 4);
    int end = spans.getInt((slot + 1) * 4);
    java.util.Objects.checkFromToIndex(start, end, head.length);
    return new String(head, start, end - start, StandardCharsets.ISO_8859_1);
  }

  private InetSocketAddress socketAddress(long socket, int kind) {
    require(api.socketAddress(driver, socket, kind, descriptorBuffer) == 0, "peer address");
    ByteBuffer view = api.bufferView(descriptorBuffer).order(ByteOrder.nativeOrder());
    byte[] bytes = new byte[view.getShort(18) == 4 ? 4 : 16];
    view.get(0, bytes);
    try {
      return new InetSocketAddress(
          InetAddress.getByAddress(bytes), Short.toUnsignedInt(view.getShort(16)));
    } catch (java.net.UnknownHostException impossible) {
      throw new IllegalStateException(impossible);
    }
  }

  private void adopt(long socket) {
    if (api.socketAdopt(owner, driver, socket) != 0) api.socketDiscard(socket);
    else if (api.socketOption(driver, socket, 1, 1) != 0
        || (tls == 0
                ? api.socketHttp(owner, driver, socket, owner, 16384)
                : api.socketHttpTls(owner, driver, socket, owner, 16384, tls))
            != 0) {
      api.socketClose(driver, socket);
    } else {
      sockets.add(socket);
      peers.put(socket, new Peer(socketAddress(socket, 0), socketAddress(socket, 1)));
    }
  }

  private void event(long operation, long socket, long value, long result, int kind) {
    if (kind == 2) {
      if (result == 0 && value != 0) {
        if (shared == null) adopt(value);
        else shared.accept(value);
      }
      require(api.socketAccept(owner, driver, listener) != 0, "accept rearm");
      return;
    }
    if (kind == TransportNative.EVENT_CLOSED) {
      sockets.remove(socket);
      peers.remove(socket);
      for (Exchange exchange : List.copyOf(exchanges.values())) {
        if (exchange.socket == socket) exchange.abort(new IOException("HTTP connection closed"));
      }
      return;
    }
    if (kind == TransportNative.EVENT_REQUEST) {
      if (shared == null ? exchanges.size() >= exchangeLimit : !shared.admitExchange()) {
        api.httpRelease(driver, value);
        api.httpFree(driver, value);
        return;
      }
      Exchange exchange = new Exchange(value, socket);
      exchanges.put(value, exchange);
      exchange.inbound.ended = (api.getByte(value + 31) & TransportNative.EXCHANGE_FLAG_BODY) == 0;
      RequestHead head = requestHead(value);
      Peer peer = peers.get(socket);
      require(peer != null, "HTTP peer metadata");
      Request request =
          new Request(
              head.method(),
              head.target(),
              head.headers(),
              exchange.inbound,
              peer.local(),
              peer.remote(),
              tls != 0,
              head.version());
      try {
        exchange.reply = handler.handle(request).toCompletableFuture();
        exchange.reply.whenComplete(
            (response, failure) -> {
              exchange.selectedResponse = response;
              if (!exchange.live || !running.get()) {
                if (response != null) response.cancel(new IOException("HTTP exchange closed"));
                return;
              }
              boolean accepted =
                  submit(
                      () -> {
                        if (!exchange.live) {
                          if (response != null)
                            response.sent.completeExceptionally(
                                new IOException("HTTP exchange closed"));
                        } else if (failure != null) exchange.abort(failure);
                        else if (response == null)
                          exchange.abort(new IOException("HTTP handler returned null"));
                        else exchange.respond(response);
                      });
              if (!accepted && response != null)
                response.cancel(new IOException("HTTP server stopped"));
            });
      } catch (Throwable failure) {
        exchange.abort(failure);
      }
      return;
    }
    Exchange exchange = exchanges.get(value);
    if (kind == TransportNative.EVENT_BODY) {
      if (exchange == null || !exchange.live) api.httpSegmentRelease(driver, operation);
      else {
        exchange.inbound.segments.addLast(operation);
        exchange.inbound.received += result;
        if (result < 0 || exchange.inbound.received > bodyLimit) {
          exchange.abort(new IOException("HTTP body limit exceeded"));
        } else exchange.inbound.drain();
      }
    } else if (exchange != null) {
      if (kind == TransportNative.EVENT_BODY_END) {
        if (result < 0) exchange.abort(new IOException("HTTP body aborted: " + result));
        else {
          exchange.inbound.ended = true;
          exchange.inbound.drain();
        }
      } else if (kind == TransportNative.EVENT_PART_SENT && exchange.outbound != null) {
        if (result < 0) exchange.abort(new IOException("HTTP response aborted: " + result));
        else exchange.outbound.sentPart();
      } else if (kind == TransportNative.EVENT_RESET || result < 0) {
        exchange.abort(new IOException("HTTP stream aborted: " + result));
      }
    }
  }

  private final class Exchange {
    final long id;
    final long socket;
    final Inbound inbound = new Inbound(this);
    volatile boolean live = true;
    volatile @Nullable Response selectedResponse;
    @Nullable CompletableFuture<Response> reply;
    @Nullable Outbound outbound;

    Exchange(long id, long socket) {
      this.id = id;
      this.socket = socket;
    }

    void respond(Response response) {
      try {
        outbound = new Outbound(this, response);
        outbound.begin();
      } catch (Throwable failure) {
        abort(failure);
      }
    }

    void abort(Throwable failure) {
      if (!live) return;
      if (outbound != null) outbound.fail(failure);
      else if (selectedResponse != null) selectedResponse.cancel(failure);
      if (reply != null && !reply.isDone()) reply.cancel(false);
      inbound.fail(failure);
      api.httpRelease(driver, id);
      finish();
    }

    void finish() {
      if (!live) return;
      live = false;
      exchanges.remove(id);
      if (shared != null) shared.exchanges.decrementAndGet();
      inbound.releaseSegments();
      api.httpFree(driver, id);
    }
  }

  private final class Inbound implements Flow.Publisher<byte[]> {
    private Executor executor() {
      return NativeHttpServer.this.executor;
    }

    final Exchange exchange;
    final ArrayDeque<Long> segments = new ArrayDeque<>();
    Flow.@Nullable Subscriber<? super byte[]> subscriber;
    long demand;
    long received;
    boolean ended;
    boolean cancelled;
    boolean subscribed;
    boolean complete;

    Inbound(Exchange exchange) {
      this.exchange = exchange;
    }

    private void reject(Flow.Subscriber<? super byte[]> next, Throwable failure) {
      next.onSubscribe(
          new Flow.Subscription() {
            @Override
            public void request(long n) {}

            @Override
            public void cancel() {}
          });
      next.onError(failure);
    }

    @Override
    public void subscribe(Flow.Subscriber<? super byte[]> next) {
      boolean accepted =
          submit(
              () -> {
                if (!exchange.live) {
                  reject(next, new IOException("HTTP exchange closed"));
                  return;
                }
                if (subscribed) {
                  next.onSubscribe(
                      new Flow.Subscription() {
                        @Override
                        public void request(long n) {}

                        @Override
                        public void cancel() {}
                      });
                  next.onError(new IllegalStateException("HTTP body permits one subscriber"));
                  return;
                }
                subscribed = true;
                subscriber = next;
                next.onSubscribe(
                    new Flow.Subscription() {
                      @Override
                      public void request(long n) {
                        submit(
                            () -> {
                              if (n <= 0)
                                exchange.abort(
                                    new IllegalArgumentException("body demand must be positive"));
                              else {
                                demand = n > Long.MAX_VALUE - demand ? Long.MAX_VALUE : demand + n;
                                drain();
                              }
                            });
                      }

                      @Override
                      public void cancel() {
                        submit(
                            () -> {
                              cancelled = true;
                              releaseSegments();
                            });
                      }
                    });
                drain();
              });
      if (!accepted) reject(next, new IOException("HTTP server stopped"));
    }

    void drain() {
      if (subscriber == null || complete) return;
      try {
        while (!cancelled && demand > 0 && !segments.isEmpty()) {
          long segment = segments.removeFirst();
          byte[] bytes;
          try {
            bytes = api.bytes(api.getLong(segment), Math.toIntExact(api.getLong(segment + 8)));
          } finally {
            api.httpSegmentRelease(driver, segment);
          }
          demand--;
          subscriber.onNext(bytes);
        }
        if (!cancelled && ended && segments.isEmpty()) {
          complete = true;
          subscriber.onComplete();
        }
      } catch (Throwable failure) {
        exchange.abort(failure);
      }
    }

    void releaseSegments() {
      while (!segments.isEmpty()) api.httpSegmentRelease(driver, segments.removeFirst());
    }

    void fail(Throwable failure) {
      boolean notify = !complete && !cancelled;
      complete = true;
      cancelled = true;
      releaseSegments();
      if (subscriber != null && notify) {
        try {
          subscriber.onError(failure);
        } catch (Throwable ignored) {
          /* User callback failed. */
        }
      }
    }
  }

  private final class Outbound implements Flow.Subscriber<byte[]> {
    final Exchange exchange;
    final Response response;
    Flow.@Nullable Subscription subscription;
    byte @Nullable [] pending;
    int pendingOffset;
    int inFlight;
    boolean requested;
    boolean ended;
    boolean finalQueued;
    boolean completeResponse;
    boolean failed;
    long prepared;
    int preparedLength;

    Outbound(Exchange exchange, Response response) {
      this.exchange = exchange;
      this.response = response;
    }

    void begin() {
      response.sent.whenComplete(
          (ignored, failure) -> {
            if (failure != null) submit(() -> exchange.abort(failure));
          });
      byte[] completeBody =
          response.body instanceof FixedBody fixed
                  && fixed.bytes.length == response.length
                  && fixed.bytes.length <= MAX_RESPONSE_PART
                  && api.supportsCompleteHttp1Responses()
              ? fixed.bytes
              : null;
      completeResponse = completeBody != null;
      long records = 0;
      long bodyAddress = 0;
      try {
        if (!response.headers.isEmpty() || completeResponse) {
          int tableBytes = Math.multiplyExact(response.headers.size(), 32);
          List<byte[]> names = new ArrayList<>();
          List<byte[]> values = new ArrayList<>();
          int capacity = Math.addExact(tableBytes, completeBody == null ? 0 : completeBody.length);
          for (Header header : response.headers) {
            byte[] name = header.name().getBytes(StandardCharsets.US_ASCII);
            byte[] value = header.value().getBytes(StandardCharsets.ISO_8859_1);
            names.add(name);
            values.add(value);
            capacity = Math.addExact(capacity, Math.addExact(name.length, value.length));
          }
          // One owner allocation holds both the records and their text. httpRespond
          // reads everything synchronously, so no per-header leases or freezes are needed.
          records = allocate(capacity);
          ByteBuffer table = api.bufferView(records).order(ByteOrder.nativeOrder());
          long base = api.bufferAddress(records);
          int offset = tableBytes;
          for (int i = 0; i < response.headers.size(); i++) {
            byte[] name = names.get(i);
            byte[] value = values.get(i);
            table.putLong(i * 32, base + offset).putLong(i * 32 + 8, name.length);
            table.put(offset, name);
            offset += name.length;
            table.putLong(i * 32 + 16, base + offset).putLong(i * 32 + 24, value.length);
            table.put(offset, value);
            offset += value.length;
          }
          if (completeBody != null) {
            bodyAddress = base + offset;
            table.put(offset, completeBody);
          }
        }
        require(
            api.httpRespond(
                    driver,
                    exchange.id,
                    response.status,
                    records == 0 ? 0 : api.bufferAddress(records),
                    response.headers.size(),
                    bodyAddress,
                    response.length,
                    completeResponse
                        ? TransportNative.RESPOND_COMPLETE_CREDIT
                        : TransportNative.RESPOND_STREAM)
                == 0,
            "response head");
        inFlight = 1;
      } finally {
        if (records != 0) api.bufferRelease(records);
      }
      if (!completeResponse) response.body.subscribe(this);
    }

    @Override
    public void onSubscribe(Flow.Subscription next) {
      if (!exchange.live || !running.get()) {
        next.cancel();
        return;
      }
      boolean accepted =
          submit(
              () -> {
                if (subscription != null || !exchange.live) next.cancel();
                else {
                  subscription = next;
                  advance();
                }
              });
      if (!accepted) next.cancel();
    }

    @Override
    public void onNext(byte[] bytes) {
      submit(
          () -> {
            if (!exchange.live) return;
            if (!requested || pending != null || ended) {
              exchange.abort(new IllegalStateException("response publisher exceeded demand"));
              return;
            }
            requested = false;
            pending = bytes;
            pendingOffset = 0;
            advance();
          });
    }

    @Override
    public void onError(Throwable failure) {
      submit(() -> exchange.abort(failure));
    }

    @Override
    public void onComplete() {
      submit(
          () -> {
            ended = true;
            requested = false;
            advance();
          });
    }

    void advance() {
      if (!exchange.live || failed || inFlight != 0) return;
      try {
        if (pending != null && pendingOffset == pending.length) {
          pending = null;
          pendingOffset = 0;
        }
        if (pending != null) {
          if (prepared == 0) {
            preparedLength = Math.min(MAX_RESPONSE_PART, pending.length - pendingOffset);
            prepared = api.httpChunkPrepare(exchange.id, preparedLength, descriptor);
            require(prepared != 0, "response part admission");
            api.writeBytes(api.getLong(descriptor), pending, pendingOffset, preparedLength);
          }
          int result = api.httpChunkSend(driver, exchange.id, prepared, preparedLength, 0);
          if (result == -2) return;
          if (result == 0 || result != -1) prepared = 0;
          require(result == 0, "response part submission: " + result);
          pendingOffset += preparedLength;
          inFlight++;
        } else if (ended) {
          if (!finalQueued) {
            if (prepared == 0) {
              prepared = api.httpChunkPrepare(exchange.id, 0, descriptor);
              require(prepared != 0, "final part admission");
            }
            int result =
                api.httpChunkSend(driver, exchange.id, prepared, 0, TransportNative.CHUNK_FINAL);
            if (result == -2) return;
            if (result == 0 || result != -1) prepared = 0;
            require(result == 0, "final part submission: " + result);
            finalQueued = true;
            inFlight++;
          } else {
            exchange.finish();
            response.sent.complete(null);
          }
        } else if (subscription != null && !requested) {
          requested = true;
          subscription.request(1);
        }
      } catch (Throwable failure) {
        exchange.abort(failure);
      }
    }

    void sentPart() {
      require(inFlight > 0, "unexpected response credit");
      inFlight--;
      if (completeResponse) {
        exchange.finish();
        response.sent.complete(null);
      } else advance();
    }

    void fail(Throwable failure) {
      if (failed) return;
      failed = true;
      if (subscription != null) {
        try {
          subscription.cancel();
        } catch (Throwable error) {
          if (failure != error) failure.addSuppressed(error);
        }
      }
      if (prepared != 0) {
        api.bufferRelease(prepared);
        prepared = 0;
      }
      response.sent.completeExceptionally(failure);
    }
  }

  private void retire() {
    IOException closing = new IOException("HTTP server stopped");
    for (Exchange exchange : List.copyOf(exchanges.values())) exchange.abort(closing);
    if (driver != 0) {
      for (long socket : sockets) api.socketClose(driver, socket);
      if (listener != 0) api.socketClose(driver, listener);
      api.httpRetire(driver);
      long deadline = System.nanoTime() + 5_000_000_000L;
      int status;
      while ((status = api.driverRelease(driver)) == -2) {
        require(System.nanoTime() < deadline, "driver retirement deadline");
        require(api.driverPoll(driver, 1_000_000L, batch, BATCH_SIZE) >= 0, "retirement poll");
      }
      require(status == 0, "driver retirement");
      sockets.clear();
      peers.clear();
    }
    if (tls != 0) api.tlsContextRelease(tls);
    if (descriptorBuffer != 0) api.bufferRelease(descriptorBuffer);
    if (batch != 0) api.bufferRelease(batch);
    if (owner != 0 && shared == null) {
      require(api.ownerUsed(owner) == 0, "native storage reclamation");
      require(api.ownerRelease(owner) == 0, "owner retirement");
    }
  }

  /** Cancel exchanges and retire I/O; completion/failure is available through {@link #stopped}. */
  @Override
  public void close() {
    if (workerGroup != null) {
      synchronized (this) {
        closed = true;
      }
      workerGroup.close();
    } else {
      stopLoop();
      awaitLoop();
    }
  }

  private void stopLoop() {
    synchronized (this) {
      closed = true;
      running.set(false);
      if (thread == null) {
        bound.completeExceptionally(
            new java.util.concurrent.CancellationException("HTTP server closed before start"));
        stopped.complete(null);
        return;
      }
    }
    long handle = wakeDriver.get();
    if (handle != 0) api.driverWake(handle);
  }

  private void awaitLoop() {
    Thread current = thread;
    if (current != null && current != Thread.currentThread()) {
      try {
        current.join(6000);
      } catch (InterruptedException interrupted) {
        Thread.currentThread().interrupt();
      }
      require(!current.isAlive(), "event loop shutdown deadline");
    }
  }
}
