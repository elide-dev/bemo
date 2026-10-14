/*
 * Copyright (c) 2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.http.NativeHttpServer;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Flow;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/** Portable worker ownership, shared admission, failure and pending-accept retirement. */
public final class NativeHttpWorkersTest {
  private static void require(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }

  private static final class Tracking extends BackendTransport {
    final AtomicInteger owners = new AtomicInteger();
    final AtomicInteger releases = new AtomicInteger();
    final AtomicInteger drivers = new AtomicInteger();
    final AtomicLong owner = new AtomicLong();
    final Set<Long> accepted = ConcurrentHashMap.newKeySet();
    final CountDownLatch acceptedTwo = new CountDownLatch(2);
    final CountDownLatch discarded = new CountDownLatch(1);
    final CountDownLatch unblock = new CountDownLatch(1);
    volatile boolean failDriver;
    volatile boolean blockWorker;
    volatile boolean blockPoll;
    final java.util.concurrent.atomic.AtomicBoolean pollClaimed =
        new java.util.concurrent.atomic.AtomicBoolean();
    final CountDownLatch pollEntered = new CountDownLatch(1);
    final CountDownLatch pollResume = new CountDownLatch(1);
    final CountDownLatch closeWake = new CountDownLatch(1);

    Tracking(TransportNative api) {
      super(api);
    }

    @Override
    public long ownerNew(long limit) {
      owners.incrementAndGet();
      long handle = super.ownerNew(limit);
      require(handle != 0 && owner.compareAndSet(0, handle), "one shared workload");
      return handle;
    }

    @Override
    public long driverNew(long workload, int backend, int limit) {
      require(workload == owner.get(), "every driver uses the shared workload");
      int ordinal = drivers.getAndIncrement();
      if (failDriver && ordinal == 1) return 0;
      if (blockWorker && Thread.currentThread().getName().equals("bemo-http-1")) {
        try {
          require(unblock.await(10, TimeUnit.SECONDS), "worker unblocked");
        } catch (InterruptedException error) {
          throw new AssertionError(error);
        }
      }
      return super.driverNew(workload, backend, limit);
    }

    @Override
    public int driverPoll(long driver, long timeout, long batch, int maximum) {
      if (blockPoll
          && Thread.currentThread().getName().equals("bemo-http-0")
          && pollClaimed.compareAndSet(false, true)) {
        pollEntered.countDown();
        try {
          require(pollResume.await(10, TimeUnit.SECONDS), "poll resumed");
        } catch (InterruptedException error) {
          throw new AssertionError(error);
        }
      }
      int count = super.driverPoll(driver, timeout, batch, maximum);
      ByteBuffer events = bufferView(batch).order(ByteOrder.nativeOrder());
      for (int i = 0; i < count; i++) {
        int offset = i * 40;
        if (events.getInt(offset + 32) == 2 && events.getLong(offset + 24) == 0) {
          long socket = events.getLong(offset + 16);
          if (socket != 0) {
            accepted.add(socket);
            acceptedTwo.countDown();
          }
        }
      }
      return count;
    }

    @Override
    public int driverWake(long driver) {
      int result = super.driverWake(driver);
      if (Thread.currentThread().getName().equals("http-worker-close-test")) closeWake.countDown();
      return result;
    }

    @Override
    public int socketAdopt(long workload, long driver, long socket) {
      int result = super.socketAdopt(workload, driver, socket);
      accepted.remove(socket);
      return result;
    }

    @Override
    public int socketDiscard(long socket) {
      int result = super.socketDiscard(socket);
      if (accepted.remove(socket)) discarded.countDown();
      return result;
    }

    @Override
    public int ownerRelease(long handle) {
      require(
          ownerUsed(handle) == 0 && accepted.isEmpty(), "all native storage and accepts retired");
      int result = super.ownerRelease(handle);
      require(result == 0 && owner.compareAndSet(handle, 0), "shared workload released once");
      releases.incrementAndGet();
      return result;
    }

    void reclaimed(int expected) {
      require(
          owners.get() == expected && releases.get() == expected && owner.get() == 0,
          "shared admission budget is not multiplied by workers");
      require(accepted.isEmpty(), "no pending accepted sockets");
    }
  }

  private static NativeHttpServer.Response response(String text) {
    byte[] bytes = text.getBytes(StandardCharsets.US_ASCII);
    Flow.Publisher<byte[]> body =
        subscriber ->
            subscriber.onSubscribe(
                new Flow.Subscription() {
                  boolean ended;

                  @Override
                  public void request(long n) {
                    require(n == 1, "one response credit");
                    if (!ended) {
                      ended = true;
                      subscriber.onNext(bytes);
                      subscriber.onComplete();
                    }
                  }

                  @Override
                  public void cancel() {
                    ended = true;
                  }
                });
    return new NativeHttpServer.Response(200, List.of(), bytes.length, body);
  }

  private static NativeHttpServer server(
      Tracking api, int port, long memory, int exchanges, NativeHttpServer.Handler handler) {
    return new NativeHttpServer(
        api, "127.0.0.1", port, memory, 1024 * 1024, exchanges, handler, null, null, 4);
  }

  private static Socket request(int port) throws Exception {
    Socket socket = new Socket("127.0.0.1", port);
    socket.setSoTimeout(5000);
    socket.getOutputStream().write("GET / HTTP/1.0\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
    return socket;
  }

  private static void participation(TransportNative backend) throws Exception {
    Tracking api = new Tracking(backend);
    Set<String> workers = ConcurrentHashMap.newKeySet();
    java.util.concurrent.atomic.AtomicReference<java.util.concurrent.Executor> executor =
        new java.util.concurrent.atomic.AtomicReference<>();
    NativeHttpServer server =
        server(
            api,
            0,
            16 * 1024 * 1024,
            32,
            request -> {
              Thread owner = Thread.currentThread();
              workers.add(owner.getName());
              executor.compareAndSet(null, request.executor());
              CompletableFuture<NativeHttpServer.Response> ready = new CompletableFuture<>();
              request
                  .executor()
                  .execute(
                      () -> {
                        try {
                          require(
                              Thread.currentThread() == owner,
                              "application callback returns to connection owner");
                          ready.complete(response("worker"));
                        } catch (Throwable failure) {
                          ready.completeExceptionally(failure);
                        }
                      });
              return ready;
            });
    try {
      int port = server.start().toCompletableFuture().get(5, TimeUnit.SECONDS);
      for (int i = 0; i < 8; i++) {
        try (Socket socket = request(port)) {
          require(
              new String(socket.getInputStream().readAllBytes(), StandardCharsets.US_ASCII)
                  .endsWith("worker"),
              "worker response");
        }
      }
      require(workers.size() == 4 && api.drivers.get() == 4, "all four workers handle requests");
    } finally {
      server.close();
    }
    server.stopped().toCompletableFuture().get(5, TimeUnit.SECONDS);
    try {
      executor
          .get()
          .execute(
              () -> {
                throw new AssertionError("closed executor ran work");
              });
      throw new AssertionError("closed executor accepted work");
    } catch (java.util.concurrent.RejectedExecutionException expected) {
      // A continuation cannot become stranded in the retired worker's task queue.
    }
    api.reclaimed(1);
  }

  private static void sharedExchangeLimit(TransportNative backend) throws Exception {
    Tracking api = new Tracking(backend);
    AtomicInteger calls = new AtomicInteger();
    CountDownLatch entered = new CountDownLatch(1);
    CompletableFuture<NativeHttpServer.Response> blocked = new CompletableFuture<>();
    NativeHttpServer server =
        server(
            api,
            0,
            16 * 1024 * 1024,
            1,
            request -> {
              if (calls.incrementAndGet() == 1) {
                entered.countDown();
                return blocked;
              }
              return CompletableFuture.completedFuture(response("next"));
            });
    try {
      int port = server.start().toCompletableFuture().get(5, TimeUnit.SECONDS);
      try (Socket first = request(port)) {
        require(entered.await(5, TimeUnit.SECONDS), "first handler admitted");
        try (Socket refused = request(port)) {
          try {
            require(refused.getInputStream().read() == -1, "over-budget connection closed");
          } catch (java.net.SocketException closed) {
            /* A reset also refuses the request. */
          }
        }
        require(calls.get() == 1, "exchange limit applies across workers");
        NativeHttpServer.Response reply = response("first");
        blocked.complete(reply);
        reply.sent().toCompletableFuture().get(5, TimeUnit.SECONDS);
        require(
            new String(first.getInputStream().readAllBytes(), StandardCharsets.US_ASCII)
                .endsWith("first"),
            "first response finishes");
      }
      try (Socket next = request(port)) {
        require(
            new String(next.getInputStream().readAllBytes(), StandardCharsets.US_ASCII)
                .endsWith("next"),
            "shared admission recovers after completion");
      }
    } finally {
      server.close();
    }
    server.stopped().toCompletableFuture().get(5, TimeUnit.SECONDS);
    api.reclaimed(1);
  }

  private static void startupFailure(TransportNative backend, boolean lowMemory) throws Exception {
    Tracking api = new Tracking(backend);
    api.failDriver = !lowMemory;
    NativeHttpServer server =
        server(
            api,
            0,
            lowMemory ? 16384 : 16 * 1024 * 1024,
            32,
            request -> CompletableFuture.completedFuture(response("unused")));
    try {
      try {
        server.start().toCompletableFuture().get(10, TimeUnit.SECONDS);
        throw new AssertionError("partial startup must fail");
      } catch (java.util.concurrent.ExecutionException
          | java.util.concurrent.CancellationException expected) {
        // Every started worker must still retire its native storage.
      }
    } finally {
      server.close();
    }
    require(
        server.stopped().toCompletableFuture().isCompletedExceptionally(),
        "startup failure reported");
    api.reclaimed(1);
  }

  private static void pendingAccepts(TransportNative backend) throws Exception {
    Tracking api = new Tracking(backend);
    api.blockWorker = true;
    int port;
    try (java.net.ServerSocket reserve = new java.net.ServerSocket(0)) {
      port = reserve.getLocalPort();
    }
    NativeHttpServer server =
        server(
            api,
            port,
            16 * 1024 * 1024,
            32,
            request -> CompletableFuture.completedFuture(response("unused")));
    server.start();
    Socket first = null, second = null;
    try {
      long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
      while (first == null) {
        try {
          first = new Socket("127.0.0.1", port);
        } catch (java.net.ConnectException notReady) {
          require(System.nanoTime() < deadline, "listener ready before last worker");
          Thread.sleep(5);
        }
      }
      second = new Socket("127.0.0.1", port);
      require(api.acceptedTwo.await(5, TimeUnit.SECONDS), "two accepted connections");
      CompletableFuture<Void> close = CompletableFuture.runAsync(server::close);
      try {
        require(
            api.discarded.await(5, TimeUnit.SECONDS),
            "queued accept discarded before worker startup");
      } finally {
        api.unblock.countDown();
      }
      close.get(10, TimeUnit.SECONDS);
    } finally {
      api.unblock.countDown();
      server.close();
      if (first != null) first.close();
      if (second != null) second.close();
    }
    server.stopped().toCompletableFuture().get(5, TimeUnit.SECONDS);
    api.reclaimed(1);
  }

  private static void queuedExecutorShutdown(TransportNative backend) throws Exception {
    Tracking api = new Tracking(backend);
    CompletableFuture<NativeHttpServer.Request> entered = new CompletableFuture<>();
    CompletableFuture<NativeHttpServer.Response> pending = new CompletableFuture<>();
    NativeHttpServer server =
        server(
            api,
            0,
            16 * 1024 * 1024,
            32,
            request -> {
              api.blockPoll = true;
              entered.complete(request);
              return pending;
            });
    AtomicInteger executed = new AtomicInteger();
    CompletableFuture<Void> closed = new CompletableFuture<>();
    try {
      int port = server.start().toCompletableFuture().get(5, TimeUnit.SECONDS);
      try (Socket client = request(port)) {
        NativeHttpServer.Request request = entered.get(5, TimeUnit.SECONDS);
        require(api.pollEntered.await(5, TimeUnit.SECONDS), "worker paused between task phases");
        request.executor().execute(executed::incrementAndGet);
        Thread closer =
            new Thread(
                () -> {
                  try {
                    server.close();
                    closed.complete(null);
                  } catch (Throwable failure) {
                    closed.completeExceptionally(failure);
                  }
                },
                "http-worker-close-test");
        closer.start();
        try {
          require(api.closeWake.await(5, TimeUnit.SECONDS), "shutdown overtook queued callback");
        } finally {
          api.pollResume.countDown();
        }
        closed.get(10, TimeUnit.SECONDS);
      }
    } finally {
      api.pollResume.countDown();
      server.close();
    }
    server.stopped().toCompletableFuture().get(5, TimeUnit.SECONDS);
    require(executed.get() == 1, "accepted callback drained exactly once during shutdown");
    api.reclaimed(1);
  }

  public static void verify(TransportNative api, Path certificate, Path key) throws Exception {
    Tracking lifecycle = new Tracking(api);
    NativeHttpServerTest.verify(lifecycle, certificate, key, 4);
    lifecycle.reclaimed(4);
    participation(api);
    queuedExecutorShutdown(api);
    sharedExchangeLimit(api);
    startupFailure(api, false);
    startupFailure(api, true);
    pendingAccepts(api);
    System.out.println(
        "Native HTTP workers: shared admission, distribution, failure and retirement passed");
  }

  public static void main(String[] args) throws Exception {
    verify(new FfmTransportNative(Path.of(args[0])), Path.of(args[1]), Path.of(args[2]));
  }
}
