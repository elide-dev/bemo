/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

import dev.elide.bemo.transport.DriverSelection;
import dev.elide.bemo.transport.TransportNative;
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
    NativeHttpBodyTest.verify(api);
    NativeGzipTest.verify(api);
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
    gatheredSend(api, driver, server, client, owner, batch, events);
    inlineSend(api, driver, server, client, owner, batch, events);
    receiveResults(api, driver, server, client, owner, batch, events, output);
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

  private static void receiveResults(
      TransportNative api,
      long driver,
      long server,
      long client,
      long owner,
      long batch,
      ByteBuffer events,
      long output) {
    check(api.supportsReceiveResults(), "receive result binding availability");
    long before = api.ownerUsed(owner);
    TransportNative.ReceiveResult pending =
        api.socketReceiveNewResult(owner, driver, server, owner, 2);
    check(pending.operation() != 0, "empty socket receive remains pending");
    check(
        pending.buffer() == 0 && pending.bytes() == null && pending.result() == 0,
        "pending receive exposes no storage");
    check(api.ownerUsed(owner) >= before + 2, "pending receive retains its capacity charge");
    check(api.socketSend(owner, driver, client, output, 0, 5) != 0, "receive result peer send");
    long operation = pending.operation();
    int received = 0;
    int sent = 0;
    int immediate = 0;
    long deadline = System.nanoTime() + 5_000_000_000L;
    while (received < 5 || sent < 5) {
      check(System.nanoTime() < deadline, "receive result deadline");
      int count = api.driverPoll(driver, 50_000_000, batch, 8);
      check(count >= 0, "receive result poll");
      for (int i = 0; i < count; i++) {
        int start = i * 40;
        int kind = events.getInt(start + 32);
        long result = events.getLong(start + 24);
        if (kind == 4) {
          check(result > 0 && result <= 5 - sent, "receive result send bounds");
          sent += (int) result;
          if (sent < 5)
            check(
                api.socketSend(owner, driver, client, output, sent, 5 - sent) != 0,
                "receive result partial send");
          continue;
        }
        check(
            kind == 3 && events.getLong(start) == operation,
            "only pending receive queues a completion");
        operation = 0;
        long handle = events.getLong(start + 16);
        received = checkReceiveResult(api, handle, result, api.bufferView(handle), received);
        while (received < 5) {
          TransportNative.ReceiveResult next =
              api.socketReceiveNewResult(owner, driver, server, owner, 2);
          if (next.operation() != 0) {
            check(next.buffer() == 0 && next.bytes() == null, "pending continuation owns no view");
            operation = next.operation();
            break;
          }
          immediate++;
          received =
              checkReceiveResult(
                  api,
                  next.buffer(),
                  next.result(),
                  java.util.Objects.requireNonNull(next.bytes()),
                  received);
        }
      }
    }
    if (api.driverBackend(driver) == 1) check(immediate > 0, "ready receive bypasses polling");
    check(api.driverPoll(driver, 0, batch, 8) == 0, "immediate receives queue no completion");
    check(api.ownerUsed(owner) == before, "receive result storage reclamation");
    check(api.socketShutdown(driver, client, 1) == 0, "receive result peer write shutdown");
    TransportNative.ReceiveResult eof = api.socketReceiveNewResult(owner, driver, server, owner, 2);
    if (eof.operation() != 0) {
      int count = api.driverPoll(driver, 5_000_000_000L, batch, 8);
      check(count == 1 && events.getLong(0) == eof.operation(), "pending EOF identity");
      check(
          events.getInt(32) == 3 && events.getLong(24) == 0 && events.getLong(16) == 0,
          "pending EOF owns no storage");
    } else {
      check(
          eof.result() == 0 && eof.buffer() == 0 && eof.bytes() == null,
          "immediate EOF owns no storage");
    }
    check(api.ownerUsed(owner) == before, "EOF storage reclamation");
  }

  private static int checkReceiveResult(
      TransportNative api, long handle, long result, ByteBuffer bytes, int received) {
    check(result > 0 && result <= Math.min(2, 5 - received), "receive result bounds");
    check(
        handle != 0 && bytes.isDirect() && !bytes.isReadOnly() && bytes.capacity() == result,
        "receive view exposes only initialized bytes");
    for (int j = 0; j < result; j++) check(bytes.get(j) == ++received, "receive result payload");
    bytes.put(0, (byte) 42);
    check(api.bufferRelease(handle) == 0, "receive result handle release");
    check(api.bufferRelease(handle) != 0, "receive result handle retires once");
    return received;
  }

  private static void inlineSend(
      TransportNative api,
      long driver,
      long server,
      long client,
      long owner,
      long batch,
      ByteBuffer events) {
    check(api.supportsInlineWrites(), "inline binding availability");
    long before = api.ownerUsed(owner);
    long sourceHandle = api.bufferNew(owner, 16);
    ByteBuffer source = api.bufferView(sourceHandle);
    source.put(new byte[] {99, 6, 7, 8, 9, 10, 99}).position(1).limit(6);
    check(
        api.socketSendInline(owner, driver, client, ByteBuffer.wrap(new byte[5])) < 0,
        "heap inline source rejection");
    check(
        api.socketSendInline(owner, driver, client, source.duplicate().limit(1)) < 0,
        "empty inline source rejection");
    check(api.socketSendInline(0, driver, client, source) < 0, "inline workload isolation");
    check(api.socketSendInline(owner, 0, client, source) < 0, "inline driver isolation");
    check(api.socketSendInline(owner, driver, 0, source) < 0, "inline socket isolation");
    long written = api.socketSendInline(owner, driver, client, source);
    check(source.position() == 1 && source.limit() == 6, "inline preserves caller geometry");
    check(api.ownerUsed(owner) == before + 16, "inline creates no private storage");
    if (api.driverBackend(driver) != 1) {
      check(written == 0, "completion backend requests asynchronous fallback");
      api.bufferRelease(sourceHandle);
      return;
    }
    check(written == 5, "inline send length");
    for (int i = 1; i < 6; i++) source.put(i, (byte) 0);
    check(api.bufferRelease(sourceHandle) == 0, "inline source released after return");
    check(api.ownerUsed(owner) == before, "inline retains no source lease");
    long receive = api.socketReceiveNew(owner, driver, server, owner, 8);
    check(receive != 0, "inline peer receive");
    int received = 0;
    long deadline = System.nanoTime() + 5_000_000_000L;
    while (received < 5) {
      check(System.nanoTime() < deadline, "inline transfer deadline");
      int count = api.driverPoll(driver, 10_000_000L, batch, 16);
      check(count >= 0, "inline transfer poll");
      for (int i = 0; i < count; i++) {
        int start = i * 40;
        check(events.getInt(start + 32) == 3, "inline creates no send completion");
        check(events.getLong(start) == receive, "inline receive identity");
        long handle = events.getLong(start + 16);
        int length = Math.toIntExact(events.getLong(start + 24));
        check(length > 0 && length <= 5 - received, "inline receive bounds");
        ByteBuffer bytes = api.bufferView(handle);
        for (int j = 0; j < length; j++)
          check(bytes.get(j) == 6 + received++, "inline payload survives mutation and release");
        check(api.bufferRelease(handle) == 0, "inline receive release");
        if (received < 5) receive = api.socketReceiveNew(owner, driver, server, owner, 8);
      }
    }
    check(api.ownerUsed(owner) == before, "inline receive reclamation");
  }

  private static void gatheredSend(
      TransportNative api,
      long driver,
      long server,
      long client,
      long owner,
      long batch,
      ByteBuffer events) {
    check(api.supportsGatheredWrites(), "gathered binding availability");
    long before = api.ownerUsed(owner);
    long first = api.bufferNew(owner, 8), second = api.bufferNew(owner, 8);
    api.bufferView(first).put(new byte[] {99, 1, 2, 3, 99});
    api.bufferView(second).put(new byte[] {99, 99, 4, 5, 99});
    check(api.bufferFreeze(first, 5) == 0, "first gathered freeze");
    long[] regions = {first, 1, 3, second, 2, 2};
    check(api.socketSendGathered(owner, driver, client, regions, 0) == 0, "empty vector rejection");
    check(
        api.socketSendGathered(owner, driver, client, regions, 65) == 0,
        "oversized vector rejection");
    check(
        api.socketSendGathered(owner, driver, client, new long[3], 2) == 0,
        "short descriptor rejection");
    check(
        api.socketSendGathered(owner, driver, client, regions, 2) == 0, "mutable region rejection");
    check(api.ownerUsed(owner) == before + 16, "rejection preserves original storage");
    check(api.bufferFreeze(second, 5) == 0, "second gathered freeze");
    check(
        api.socketSendGathered(owner, driver, client, new long[] {first, Long.MAX_VALUE, 3}, 1)
            == 0,
        "region bounds rejection");
    check(api.socketSendGathered(0, driver, client, regions, 2) == 0, "workload isolation");
    long receive = api.socketReceiveNew(owner, driver, server, owner, 8);
    check(receive != 0, "gathered peer receive");
    long afterReceive = api.ownerUsed(owner);
    long send = api.socketSendGathered(owner, driver, client, regions, 2);
    check(send != 0, "gathered send admission");
    check(
        api.bufferRelease(first) == 0 && api.bufferRelease(second) == 0,
        "release submitted handles");
    check(api.ownerUsed(owner) == afterReceive, "kernel leases retain submitted storage");
    java.util.Arrays.fill(regions, 0); // The asynchronous operation must not borrow descriptors.
    int received = 0;
    boolean sent = false;
    long deadline = System.nanoTime() + 5_000_000_000L;
    while (received < 5 || !sent) {
      check(System.nanoTime() < deadline, "gathered transfer deadline");
      int count = api.driverPoll(driver, 10_000_000L, batch, 16);
      check(count >= 0, "gathered transfer poll");
      for (int i = 0; i < count; i++) {
        int start = i * 40;
        int kind = events.getInt(start + 32);
        if (kind == 4) {
          check(events.getLong(start) == send, "gathered completion identity");
          check(events.getLong(start + 24) == 5 && !sent, "exact gathered send completion");
          sent = true;
        } else if (kind == 3) {
          check(events.getLong(start) == receive, "gathered receive identity");
          long handle = events.getLong(start + 16);
          int length = Math.toIntExact(events.getLong(start + 24));
          check(length > 0 && length <= 5 - received, "gathered receive bounds");
          ByteBuffer bytes = api.bufferView(handle);
          for (int j = 0; j < length; j++)
            check(bytes.get(j) == ++received, "gathered payload order");
          check(api.bufferRelease(handle) == 0, "gathered receive release");
          if (received < 5) receive = api.socketReceiveNew(owner, driver, server, owner, 8);
        } else throw new AssertionError("unexpected gathered completion " + kind);
      }
    }
    check(api.ownerUsed(owner) == before, "gathered lease reclamation");
    check(
        api.socketSendGathered(owner, driver, client, new long[] {first, 0, 1}, 1) == 0,
        "stale gathered handle rejection");
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
