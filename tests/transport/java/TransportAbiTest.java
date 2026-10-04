/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

import dev.elide.netty.v2.DriverSelection;
import dev.elide.netty.v2.TransportNative;
import java.nio.ByteBuffer;

/** Shared behavioral checks for the FFM and Native Image binding implementations. */
public final class TransportAbiTest {

  public static void verify(TransportNative api) throws Exception {
    check(api.version() == 3 && TransportNative.ABI_VERSION == 3, "ABI version");
    long owner = api.ownerNew(16);
    long buffer = api.bufferNew(owner, 16);
    check(owner != 0 && buffer != 0, "allocation");
    check(api.ownerUsed(owner) == 16, "budget charge");
    check(api.bufferNew(owner, 1) == 0, "budget enforcement");
    ByteBuffer bytes = api.bufferView(buffer);
    check(bytes.capacity() == 16 && bytes.get(15) == 0, "initialized capacity");
    check(api.bufferCapacity(buffer) == 16, "mutable descriptor capacity");
    bytes.put(0, (byte) 42);
    check(api.bufferFreeze(buffer, 1) == 0, "freeze");
    long slice = api.bufferSlice(buffer, 0, 1);
    check(slice != 0, "retained slice");
    check(api.bufferRelease(buffer) == 0, "release parent");
    ByteBuffer retained = api.bufferView(slice);
    check(retained.isReadOnly() && retained.get(0) == 42, "retained native storage");
    check(api.bufferCapacity(slice) == 1, "frozen descriptor capacity");
    check(api.ownerUsed(owner) == 16, "retained charge");
    check(api.bufferRelease(buffer) == -1, "stale handle rejection");
    check(api.bufferRelease(slice) == 0, "release slice");
    check(api.ownerUsed(owner) == 0, "reclamation");
    long driver = api.driverNew(owner, 0, 8);
    check(driver != 0, "driver initialization");
    check(api.driverBackend(driver) > 0, "actual backend");
    driverSelection(api, driver);
    Thread thread =
        new Thread(
            () -> {
              check(api.driverBackend(driver) == -1, "thread affinity");
              check(api.driverFallback(driver, 0) == -1, "fallback thread affinity");
              check(api.driverWake(driver) == 0, "cross-thread wakeup");
            });
    thread.setUncaughtExceptionHandler(
        (ignored, error) -> {
          error.printStackTrace();
          System.exit(1);
        });
    thread.start();
    thread.join();
    check(api.driverRelease(driver) == 0, "driver release");
    check(api.driverWake(driver) == -1, "released wakeup");
    check(api.ownerRelease(owner) == 0, "owner release");
    concurrentViews(api);
    sockets(api);
    workloads(api);
    System.out.println("Transport ABI checks passed");
  }

  private static void concurrentViews(TransportNative api) throws Exception {
    long owner = api.ownerNew(4096);
    var barrier = new java.util.concurrent.CyclicBarrier(4);
    try (var workers = java.util.concurrent.Executors.newFixedThreadPool(4)) {
      var tasks = new java.util.ArrayList<java.util.concurrent.Callable<Void>>();
      for (int worker = 0; worker < 4; worker++) {
        int capacity = 16 + worker;
        tasks.add(
            () -> {
              for (int iteration = 0; iteration < 32; iteration++) {
                long handle = api.bufferNew(owner, capacity);
                check(handle != 0, "concurrent allocation");
                try {
                  barrier.await(5, java.util.concurrent.TimeUnit.SECONDS);
                  ByteBuffer first = api.bufferView(handle);
                  check(
                      first.capacity() == capacity && !first.isReadOnly(),
                      "independent descriptor");
                  first.put(0, (byte) capacity);
                  ByteBuffer second = api.bufferView(handle);
                  check(
                      second.get(0) == capacity && first.get(0) == capacity,
                      "descriptor reuse preserves views");
                  check(api.bufferFreeze(handle, 1) == 0, "concurrent freeze");
                  ByteBuffer frozen = api.bufferView(handle);
                  check(
                      frozen.capacity() == 1 && frozen.isReadOnly() && frozen.get(0) == capacity,
                      "descriptor reflects ownership transition");
                } finally {
                  api.bufferRelease(handle);
                }
              }
              return null;
            });
      }
      for (var future : workers.invokeAll(tasks)) future.get();
    } finally {
      check(api.ownerUsed(owner) == 0, "concurrent allocations reclaimed");
      api.ownerRelease(owner);
    }
  }

  private static void sockets(TransportNative api) {
    long owner = api.ownerNew(65536);
    long address = api.bufferNew(owner, 24);
    ByteBuffer endpoint = api.bufferView(address).order(java.nio.ByteOrder.nativeOrder());
    endpoint.put(0, (byte) 127).put(3, (byte) 1).putShort(18, (short) 4);
    check(api.bufferFreeze(address, 24) == 0, "endpoint freeze");
    long driver = api.driverNew(owner, 0, 16);
    long listener = api.socketListen(owner, driver, address, 16, 1);
    check(listener != 0, "native listener");
    api.bufferRelease(address);
    address = api.bufferNew(owner, 24);
    check(api.socketAddress(driver, listener, 0, address) == 0, "listener address");
    check(api.bufferFreeze(address, 24) == 0, "bound endpoint");
    check(api.socketAccept(owner, driver, listener) != 0, "accept submission");
    long client = api.socketConnect(owner, driver, address);
    check(client != 0, "connect submission");
    api.bufferRelease(address);
    long batch = api.bufferNew(owner, 16 * 40);
    ByteBuffer events = api.bufferView(batch).order(java.nio.ByteOrder.nativeOrder());
    long server = 0;
    boolean connected = false;
    long deadline = System.nanoTime() + 5_000_000_000L;
    while (server == 0 || !connected) {
      check(System.nanoTime() < deadline, "connect deadline");
      int count = api.driverPoll(driver, 10_000_000L, batch, 16);
      check(count >= 0, "connect poll");
      for (int i = 0; i < count; i++) {
        int start = i * 40;
        check(events.getLong(start + 24) == 0, "connect/accept success");
        if (events.getInt(start + 32) == 1) connected = true;
        if (events.getInt(start + 32) == 2) server = events.getLong(start + 16);
      }
    }
    check(api.socketAdopt(owner, driver, server) == 0, "accepted adoption");
    long input = api.bufferNew(owner, 1);
    check(api.socketReceive(owner, driver, server, input) != 0, "receive submission");
    long rejected = api.bufferNew(owner, 16);
    check(rejected != 0, "rejected receive storage allocation");
    api.bufferView(rejected).put(0, (byte) 42);
    long charged = api.ownerUsed(owner);
    check(
        api.socketReceive(owner, driver, server, rejected) == 0, "pending receive admission limit");
    ByteBuffer restored = api.bufferView(rejected);
    check(restored.capacity() == 16 && restored.get(0) == 42, "failed receive restores storage");
    restored.put(1, (byte) 7);
    check(api.ownerUsed(owner) == charged, "failed receive preserves budget charge");
    check(api.bufferRelease(rejected) == 0, "failed receive handle remains owned");
    long output = api.bufferNew(owner, 5);
    api.bufferView(output).put(new byte[] {1, 2, 3, 4, 5});
    check(api.bufferFreeze(output, 5) == 0, "send freeze");
    check(api.socketSend(owner, driver, client, output, 0, 5) != 0, "send submission");
    int received = 0;
    int sent = 0;
    while (received < 5 || sent < 5) {
      check(System.nanoTime() < deadline, "transfer deadline");
      int count = api.driverPoll(driver, 10_000_000L, batch, 16);
      check(count >= 0, "transfer poll");
      for (int i = 0; i < count; i++) {
        int start = i * 40;
        long transferred = events.getLong(start + 24);
        check(transferred > 0 && transferred <= 5, "transferred bytes");
        if (events.getInt(start + 32) == 3) {
          check(events.getLong(start + 16) == input, "receive storage identity");
          check(transferred == 1, "bounded receive");
          check(api.bufferView(input).get(0) == ++received, "received payload order");
          if (received < 5) {
            api.bufferRelease(input);
            input = api.bufferNew(owner, 1);
            check(
                api.socketReceive(owner, driver, server, input) != 0,
                "partial receive resubmission");
          }
        }
        if (events.getInt(start + 32) == 4) {
          sent += (int) transferred;
          check(sent <= 5, "send bounds");
          if (sent < 5)
            check(
                api.socketSend(owner, driver, client, output, sent, 5 - sent) != 0,
                "partial send resubmission");
        }
      }
    }
    allocatedReceive(api, driver, server, client, owner, batch, events, output);
    api.socketClose(driver, server);
    api.socketClose(driver, client);
    api.socketClose(driver, listener);
    check(api.driverRelease(driver) == 0, "socket driver shutdown");
    api.bufferRelease(input);
    api.bufferRelease(output);
    api.bufferRelease(batch);
    check(api.ownerUsed(owner) == 0, "socket buffers reclaimed");
    api.ownerRelease(owner);
  }

  private static void allocatedReceive(
      TransportNative api,
      long driver,
      long server,
      long client,
      long owner,
      long batch,
      ByteBuffer events,
      long output) {
    long tiny = api.ownerNew(8);
    check(api.socketReceiveNew(owner, driver, server, tiny, 16) == 0, "receive allocation budget");
    check(api.ownerUsed(tiny) == 0, "rejected receive leaves no allocation");
    api.ownerRelease(tiny);
    long before = api.ownerUsed(owner);
    long operation = api.socketReceiveNew(owner, driver, server, owner, 16);
    check(
        operation != 0 && api.ownerUsed(owner) == before + 16, "native receive allocation charge");
    check(api.socketReceiveNew(owner, driver, server, owner, 32) == 0, "native receive lane limit");
    check(api.ownerUsed(owner) == before + 16, "rejected native receive releases allocation");
    check(
        api.socketSend(owner, driver, client, output, 0, 1) != 0,
        "native allocated receive peer send");
    boolean received = false;
    boolean sent = false;
    long deadline = System.nanoTime() + 5_000_000_000L;
    while (!received || !sent) {
      check(System.nanoTime() < deadline, "native allocated receive deadline");
      int count = api.driverPoll(driver, 10_000_000L, batch, 16);
      check(count >= 0, "native allocated receive poll");
      for (int i = 0; i < count; i++) {
        int start = i * 40;
        check(events.getLong(start + 24) == 1, "native allocated transfer size");
        if (events.getInt(start + 32) == 3) {
          check(events.getLong(start) == operation, "native allocated receive identity");
          long handle = events.getLong(start + 16);
          ByteBuffer view = api.bufferView(handle);
          check(view.capacity() == 1 && view.get(0) == 1, "only initialized receive bytes exposed");
          check(api.bufferFreeze(handle, 2) == -1, "uninitialized receive tail is inaccessible");
          check(api.bufferFreeze(handle, 1) == 0, "native receive freeze");
          long retained = api.bufferSlice(handle, 0, 1);
          check(retained != 0 && api.bufferRelease(handle) == 0, "native receive retained lease");
          check(api.ownerUsed(owner) == before + 16, "retained receive charges full allocation");
          check(api.bufferView(retained).get(0) == 1, "retained receive payload");
          api.bufferRelease(retained);
          received = true;
        } else if (events.getInt(start + 32) == 4) {
          sent = true;
        }
      }
    }
    check(api.ownerUsed(owner) == before, "native receive allocation reclaimed");
  }

  /**
   * Forced backends never report a fallback; Linux AUTO names the io_uring refusal behind polling.
   */
  private static void driverSelection(TransportNative api, long driver) {
    long owner = api.ownerNew(1024);
    DriverSelection selection = DriverSelection.of(api, driver, owner, 0);
    DriverSelection observed = DriverSelection.observe(api, driver, owner, 0);
    check(observed.equals(selection), "event-loop record matches the full query: " + observed);
    check(observed == DriverSelection.observed(), "event-loop record published");
    if (observed.fallback() == null)
      check(
          DriverSelection.observe(api, driver, owner, 0) == observed, "unchanged selection reused");
    check(api.ownerRelease(owner) == 0, "fallback reason storage released");
    String backend = System.getenv("ELIDE_TRANSPORT_TEST_BACKEND");
    if ((backend != null && !backend.equals("auto"))
        || !System.getProperty("os.name").equals("Linux"))
      check(selection.fallback() == null, "only Linux AUTO falls back: " + selection.describe());
    else
      check(
          (selection.actual() == 1) == (selection.fallback() != null),
          "AUTO fallback reason: " + selection.describe());
    String errno = System.getenv("ELIDE_TRANSPORT_TEST_EXPECT_FALLBACK");
    if (errno != null)
      check(
          selection.fallback() != null && selection.fallback().endsWith("(os error " + errno + ")"),
          "expected io_uring refusal: " + selection.describe());
    System.out.println("Verified driver selection: " + selection.describe());
  }

  /** Two workloads share one driver; closing one cancels its work and leaves the other serving. */
  private static void workloads(TransportNative api) {
    long closing = api.ownerNew(65536);
    long serving = api.ownerNew(65536);
    long driver = api.driverNew(serving, 0, 16);
    long batch = api.bufferNew(serving, 16 * 40);
    ByteBuffer events = api.bufferView(batch).order(java.nio.ByteOrder.nativeOrder());
    long address = api.bufferNew(serving, 24);
    api.bufferView(address)
        .order(java.nio.ByteOrder.nativeOrder())
        .put(0, (byte) 127)
        .put(3, (byte) 1)
        .putShort(18, (short) 4);
    check(api.bufferFreeze(address, 24) == 0, "workload endpoint freeze");
    long listener = api.socketListen(serving, driver, address, 16, 1);
    check(listener != 0, "workload listener");
    api.bufferRelease(address);
    address = api.bufferNew(serving, 24);
    check(api.socketAddress(driver, listener, 0, address) == 0, "workload listener address");
    check(api.bufferFreeze(address, 24) == 0, "workload bound endpoint");
    long doomed = api.socketConnect(closing, driver, address);
    long kept = api.socketConnect(serving, driver, address);
    check(doomed != 0 && kept != 0, "connects under two workloads");
    api.bufferRelease(address);
    check(api.socketAccept(closing, driver, listener) == 0, "listener keeps its workload");
    long[] peers = new long[2];
    for (int i = 0; i < peers.length; i++) {
      long accept = api.socketAccept(serving, driver, listener);
      check(accept != 0, "workload accept");
      long[] accepted = await(api, driver, batch, events, accept);
      check(accepted[1] == 0 && accepted[0] != 0, "workload accept result");
      check(
          api.socketAdopt(closing, driver, accepted[0]) == -1,
          "adoption keeps the listener workload");
      check(api.socketAdopt(serving, driver, accepted[0]) == 0, "workload adoption");
      peers[i] = accepted[0];
    }
    check(
        api.socketReceiveNew(serving, driver, doomed, serving, 16) == 0,
        "foreign workload receive");
    long pending = api.socketReceiveNew(closing, driver, doomed, closing, 16);
    check(pending != 0, "closing workload receive");
    check(api.workloadClose(closing) == 0, "workload close");
    check(await(api, driver, batch, events, pending)[1] < 0, "closed workload receive cancelled");
    check(
        api.socketReceiveNew(closing, driver, doomed, closing, 16) == 0,
        "closed workload admits nothing");
    long output = api.bufferNew(serving, 1);
    api.bufferView(output).put(0, (byte) 9);
    check(api.bufferFreeze(output, 1) == 0, "workload payload freeze");
    check(api.socketSend(closing, driver, doomed, output, 0, 1) == 0, "closed workload send");
    long receive = api.socketReceiveNew(serving, driver, kept, serving, 16);
    check(receive != 0, "surviving workload receive");
    for (long peer : peers)
      check(api.socketSend(serving, driver, peer, output, 0, 1) != 0, "surviving workload send");
    long[] received = await(api, driver, batch, events, receive);
    check(
        received[1] == 1 && api.bufferView(received[0]).get(0) == 9, "surviving workload payload");
    api.bufferRelease(received[0]);
    for (long socket : new long[] {doomed, kept, listener, peers[0], peers[1]})
      api.socketClose(driver, socket);
    int status;
    while ((status = api.driverRelease(driver)) == -2)
      api.driverPoll(driver, 1_000_000L, batch, 16);
    check(status == 0, "workload driver shutdown");
    api.bufferRelease(output);
    api.bufferRelease(batch);
    check(api.ownerUsed(closing) == 0 && api.ownerUsed(serving) == 0, "workload storage reclaimed");
    check(api.ownerRelease(closing) == 0 && api.ownerRelease(serving) == 0, "workload release");
  }

  /** Poll until `operation` completes, returning its value and result; drops unrelated payloads. */
  private static long[] await(
      TransportNative api, long driver, long batch, ByteBuffer events, long operation) {
    long deadline = System.nanoTime() + 5_000_000_000L;
    while (true) {
      check(System.nanoTime() < deadline, "workload completion deadline");
      int count = api.driverPoll(driver, 10_000_000L, batch, 16);
      check(count >= 0, "workload poll");
      for (int i = 0; i < count; i++) {
        int start = i * 40;
        long value = events.getLong(start + 16);
        if (events.getLong(start) == operation)
          return new long[] {value, events.getLong(start + 24)};
        if (events.getInt(start + 32) == 3 && value != 0) api.bufferRelease(value);
      }
    }
  }

  private static void check(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }
}
