/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.dokar.transport;

import io.netty.channel.*;
import io.netty.util.collection.LongObjectHashMap;
import io.netty.util.concurrent.ThreadAwareExecutor;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;

/** Embeds one Rust completion driver on each Netty I/O thread. */
public final class NativeIoHandler implements IoHandler {
  @SuppressWarnings("try")
  interface Handle extends IoHandle {}

  static final class Completion implements IoEvent {
    long operation;
    long socket;
    long value;
    long result;
    int kind;
  }

  final TransportNative api;
  private final ThreadAwareExecutor executor;
  private final int backend;
  private final int limit;
  private final long memoryLimit;
  private final Workload.Profile profile;
  private final long suppliedWorkload;
  private final Set<Registration> registrations = new HashSet<>();
  private final LongObjectHashMap<Registration> sockets = new LongObjectHashMap<>();
  private final Completion completion = new Completion();
  private volatile long driver;
  private long workload;
  private long owner;
  private long batch;
  private ByteBuffer events;
  NativeByteBufAllocator allocator;
  private long iteration;
  private String backendName;

  private NativeIoHandler(
      ThreadAwareExecutor executor,
      TransportNative api,
      int backend,
      int limit,
      long memoryLimit,
      Workload.Profile profile,
      long suppliedWorkload) {
    this.executor = executor;
    this.api = api;
    this.backend = backend;
    this.limit = limit;
    this.memoryLimit = memoryLimit;
    this.profile = profile;
    this.suppliedWorkload = suppliedWorkload;
  }

  public static IoHandlerFactory newFactory(
      TransportNative api, int backend, int limit, long memoryLimit) {
    return newFactory(api, backend, limit, memoryLimit, Workload.DEFAULT);
  }

  /** Drivers and the channels they register belong to {@code profile}'s workload by default. */
  public static IoHandlerFactory newFactory(
      TransportNative api, int backend, int limit, long memoryLimit, Workload.Profile profile) {
    if (limit < 1 || limit > 65536 || memoryLimit < limit * 40L)
      throw new IllegalArgumentException("Invalid driver limits");
    java.util.Objects.requireNonNull(profile);
    return executor -> new NativeIoHandler(executor, api, backend, limit, memoryLimit, profile, 0);
  }

  /**
   * Share an explicit workload's allocation ceiling across drivers; the caller owns its lifetime.
   */
  public static IoHandlerFactory forWorkload(
      TransportNative api, int backend, int limit, long workload) {
    if (workload == 0 || limit < 1 || limit > 65536)
      throw new IllegalArgumentException("Invalid workload or driver limit");
    return executor ->
        new NativeIoHandler(executor, api, backend, limit, 0, Workload.DEFAULT, workload);
  }

  @Override
  public void initialize() {
    if (executor instanceof io.netty.util.concurrent.SingleThreadEventExecutor loop
        && loop.isShuttingDown()) return;
    workload = suppliedWorkload == 0 ? Workload.id(api, profile) : suppliedWorkload;
    owner = suppliedWorkload == 0 ? api.ownerNew(memoryLimit) : suppliedWorkload;
    batch = api.bufferNew(owner, limit * 40L);
    driver = api.driverNew(workload, backend, limit);
    int status = driver == 0 ? api.lastError() : 0;
    if (owner == 0 || batch == 0 || driver == 0) {
      if (driver != 0) api.driverRelease(driver);
      if (batch != 0) api.bufferRelease(batch);
      if (owner != 0 && suppliedWorkload == 0) api.ownerRelease(owner);
      throw new IllegalStateException(
          "Native transport initialization failed",
          status == 0 ? null : NativeTransportException.operation("driver", status));
    }
    backendName = DriverSelection.observe(api, driver, owner, backend).driver();
    events = api.bufferView(batch).order(ByteOrder.nativeOrder());
    allocator = new NativeByteBufAllocator(api, owner);
    if (executor instanceof io.netty.util.concurrent.SingleThreadEventExecutor loop)
      loop.addShutdownHook(this::prepareToDestroy);
  }

  @Override
  public int run(IoHandlerContext context) {
    if (driver == 0) return 0;
    long started = System.nanoTime();
    long timeout = context.canBlock() ? Math.max(0, context.delayNanos(started)) : 0;
    TransportEvents.Batch flight = TransportEvents.batch(driver, backendName, limit, timeout);
    long pollStarted = flight == null ? 0 : System.nanoTime();
    int count = api.driverPoll(driver, timeout, batch, limit);
    long dispatchStart = System.nanoTime();
    if (flight != null) {
      flight.pollNanos = dispatchStart - pollStarted;
      flight.completions = Math.max(0, count);
    }
    if (count < 0) {
      if (flight != null) {
        flight.errors = 1;
        flight.end();
        flight.commit();
      }
      throw new IllegalStateException("Native transport poll failed");
    }
    int active = 0;
    iteration++;
    try {
      for (int i = 0; i < count; i++) {
        int offset = i * 40;
        completion.operation = events.getLong(offset);
        completion.socket = events.getLong(offset + 8);
        completion.value = events.getLong(offset + 16);
        completion.result = events.getLong(offset + 24);
        completion.kind = events.getInt(offset + 32);
        if (flight != null) {
          if (completion.result < 0) flight.errors++;
          switch (completion.kind) {
            case 1 -> flight.connects++;
            case 2 -> flight.accepts++;
            case 3 -> {
              flight.reads++;
              flight.receivedBytes += Math.max(0, completion.result);
            }
            case 4 -> {
              flight.writes++;
              flight.writtenBytes += Math.max(0, completion.result);
            }
            default -> {}
          }
        }
        Registration registration = sockets.get(completion.socket);
        try {
          if (registration != null && registration.isValid()) {
            if (registration.iteration != iteration) {
              registration.iteration = iteration;
              active++;
            }
            registration.handle.handle(registration, completion);
          }
        } finally {
          if (completion.value != 0) {
            if (completion.kind == 2) api.socketDiscard(completion.value);
            if (completion.kind == 3) api.bufferRelease(completion.value);
          }
        }
      }
      return active;
    } finally {
      long dispatchNanos = System.nanoTime() - dispatchStart;
      if (context.shouldReportActiveIoTime()) context.reportActiveIoTime(dispatchNanos);
      if (flight != null) {
        flight.end();
        if (flight.shouldCommit()) {
          flight.activeChannels = active;
          flight.dispatchNanos = dispatchNanos;
          flight.nativeBytes = api.ownerUsed(owner);
          flight.commit();
        }
      }
    }
  }

  @Override
  public void wakeup() {
    long current = driver;
    if (current != 0) api.driverWake(current);
  }

  @Override
  public boolean isCompatible(Class<? extends IoHandle> type) {
    return Handle.class.isAssignableFrom(type);
  }

  @Override
  public IoRegistration register(IoHandle handle) {
    if (driver == 0) throw new IllegalStateException("Native transport is closed");
    if (!(handle instanceof Handle nativeHandle))
      throw new IllegalArgumentException("Native handle required");
    Registration registration = new Registration(nativeHandle);
    registrations.add(registration);
    handle.registered();
    return registration;
  }

  @Override
  public void prepareToDestroy() {
    for (Registration registration : Set.copyOf(registrations)) {
      try {
        registration.handle.close();
      } catch (Exception error) {
        throw new IllegalStateException(error);
      }
    }
  }

  @Override
  public void destroy() {
    long current = driver;
    if (current == 0) return;
    TransportEvents.Shutdown flight = TransportEvents.shutdown(current, backendName);
    // Shutdown can win after the loop's prepare check but before confirmShutdown.
    prepareToDestroy();
    int retries = 0;
    int status;
    do {
      status = api.driverRelease(current);
      if (status == -2) {
        retries++;
        java.util.concurrent.locks.LockSupport.parkNanos(100_000);
      }
    } while (status == -2);
    if (status != 0) {
      if (flight != null) {
        flight.status = status;
        flight.busyRetries = retries;
        flight.end();
        flight.commit();
      }
      throw new IllegalStateException("Native driver cleanup failed");
    }
    driver = 0;
    api.bufferRelease(batch);
    if (flight != null) {
      flight.end();
      if (flight.shouldCommit()) {
        flight.busyRetries = retries;
        flight.retainedBytes = api.ownerUsed(owner);
        flight.commit();
      }
    }
    if (suppliedWorkload == 0) api.ownerRelease(owner);
    registrations.clear();
    sockets.clear();
  }

  void associate(long socket, IoRegistration registration) {
    sockets.put(socket, registration.attachment());
  }

  long driver() {
    return driver;
  }

  long workload() {
    return workload;
  }

  long unixEndpoint(java.net.UnixDomainSocketAddress address) {
    byte[] path = address.getPath().toString().getBytes(java.nio.charset.StandardCharsets.UTF_8);
    if (path.length == 0) throw new IllegalArgumentException("Unix socket path required");
    for (byte value : path)
      if (value == 0) throw new IllegalArgumentException("NUL in Unix socket path");
    long handle = api.bufferNew(owner, 24 + path.length);
    if (handle == 0) throw new OutOfMemoryError("Native endpoint budget exhausted");
    try {
      ByteBuffer bytes = api.bufferView(handle).order(ByteOrder.nativeOrder());
      bytes.putLong(0).putLong(0).putLong(0);
      bytes.putShort(18, (short) 1);
      bytes.put(path);
      if (api.bufferFreeze(handle, 24 + path.length) != 0)
        throw new IllegalStateException("Endpoint encoding failed");
      return handle;
    } catch (RuntimeException | Error error) {
      api.bufferRelease(handle);
      throw error;
    }
  }

  long endpoint(InetSocketAddress address) {
    if (address.isUnresolved()) throw new IllegalArgumentException("Resolved IP endpoint required");
    long handle = api.bufferNew(owner, 24);
    if (handle == 0) throw new OutOfMemoryError("Native endpoint budget exhausted");
    try {
      ByteBuffer bytes = api.bufferView(handle).order(ByteOrder.nativeOrder());
      bytes.put(address.getAddress().getAddress());
      bytes.putShort(16, (short) address.getPort());
      bytes.putShort(18, (short) (address.getAddress() instanceof Inet4Address ? 4 : 6));
      if (address.getAddress() instanceof Inet6Address ipv6) bytes.putInt(20, ipv6.getScopeId());
      if (api.bufferFreeze(handle, 24) != 0)
        throw new IllegalStateException("Endpoint encoding failed");
      return handle;
    } catch (RuntimeException | Error error) {
      api.bufferRelease(handle);
      throw error;
    }
  }

  InetSocketAddress address(long socket, boolean peer) {
    long handle = api.bufferNew(owner, 24);
    if (handle == 0) throw new OutOfMemoryError("Native endpoint budget exhausted");
    try {
      if (api.socketAddress(driver, socket, peer ? 1 : 0, handle) != 0) return null;
      ByteBuffer bytes = api.bufferView(handle).order(ByteOrder.nativeOrder());
      int family = bytes.getShort(18);
      byte[] octets = new byte[family == 4 ? 4 : 16];
      bytes.get(octets);
      InetAddress ip =
          family == 4
              ? InetAddress.getByAddress(octets)
              : Inet6Address.getByAddress(null, octets, bytes.getInt(20));
      return new InetSocketAddress(ip, Short.toUnsignedInt(bytes.getShort(16)));
    } catch (java.net.UnknownHostException error) {
      throw new AssertionError(error);
    } finally {
      api.bufferRelease(handle);
    }
  }

  void close(long socket) {
    sockets.remove(socket);
    api.socketClose(driver, socket);
  }

  final class Registration implements IoRegistration {
    final NativeIoHandler owner = NativeIoHandler.this;
    private final Handle handle;
    private final AtomicBoolean cancelled = new AtomicBoolean();
    private long iteration;

    Registration(Handle handle) {
      this.handle = handle;
    }

    @Override
    @SuppressWarnings("unchecked")
    public <T> T attachment() {
      return (T) this;
    }

    @Override
    public long submit(IoOps ops) {
      throw new UnsupportedOperationException("Use native channel operations");
    }

    @Override
    public boolean isValid() {
      return !cancelled.get();
    }

    @Override
    public boolean cancel() {
      if (!cancelled.compareAndSet(false, true)) return false;
      if (executor.isExecutorThread(Thread.currentThread())) unregister();
      else executor.execute(this::unregister);
      return true;
    }

    private void unregister() {
      registrations.remove(this);
      handle.unregistered();
    }
  }
}
