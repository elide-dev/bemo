/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.bemo.transport;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.ByteBuffer;
import java.nio.file.Path;
import org.jspecify.annotations.Nullable;

/**
 * Traditional JVM bindings to the standalone library; the library remains loaded for process life.
 */
@SuppressWarnings("restricted")
public final class FfmTransportNative implements TransportNative {
  // Downcalls never call Java; each descriptor is consumed before the next call on this thread.
  private static final ThreadLocal<MemorySegment> BUFFER_DESCRIPTOR =
      ThreadLocal.withInitial(() -> Arena.ofAuto().allocate(32, 8));

  // One process-wide unbounded view of the address space, created once: a scalar read off it costs
  // no allocation, where a per-read `ofAddress(..).reinterpret(..)` or `asByteBuffer()` costs one.
  private static final MemorySegment ALL = MemorySegment.NULL.reinterpret(Long.MAX_VALUE);

  private final MethodHandle version;
  private final MethodHandle lastError;
  private final MethodHandle ownerNew;
  private final MethodHandle ownerUsed;
  private final MethodHandle ownerRelease;
  private final MethodHandle workloadClose;
  private final MethodHandle gzipNew;
  private final MethodHandle gzipCompress;
  private final MethodHandle gzipRelease;
  private final MethodHandle bufferNew;
  private final MethodHandle bufferView;
  private final MethodHandle bufferFreeze;
  private final MethodHandle bufferSlice;
  private final MethodHandle bufferRelease;
  private final MethodHandle driverNew;
  private final MethodHandle driverBackend;
  private final MethodHandle driverWake;
  private final MethodHandle driverRelease;
  private final @Nullable MethodHandle driverFallback;

  private final MethodHandle servingNew;
  private final MethodHandle servingSplitNew;
  private final MethodHandle servingWorkers;
  private final MethodHandle servingWorkerDriver;
  private final MethodHandle servingAvailableCores;
  private final MethodHandle servingContextEnter;
  private final MethodHandle servingContextLeave;
  private final MethodHandle servingDriver;
  private final MethodHandle servingListen;
  private final MethodHandle servingReady;
  private final MethodHandle servingCpu;
  private final MethodHandle servingListenerClose;
  private final MethodHandle servingClose;

  private final MethodHandle socketListen;
  private final MethodHandle socketConnect;
  private final MethodHandle socketAccept;
  private final MethodHandle socketAdopt;
  private final MethodHandle socketDiscard;
  private final MethodHandle socketAddress;
  private final MethodHandle socketReceive;
  private final MethodHandle socketReceiveNew;
  private final @Nullable MethodHandle socketReceiveNewResult;
  private final MethodHandle socketSend;
  private final @Nullable MethodHandle socketSendInline;
  private final @Nullable MethodHandle socketSendGathered;
  private static final ThreadLocal<MemorySegment> SEND_REGIONS =
      ThreadLocal.withInitial(() -> Arena.ofAuto().allocate(64 * 24, 8));
  private final MethodHandle socketClose;
  private final MethodHandle driverPoll;
  private final MethodHandle socketHttp;
  private final MethodHandle socketHttpTls;
  private final MethodHandle httpMethod;
  private final MethodHandle httpVersion;
  private final MethodHandle httpHeaderCount;
  private final MethodHandle httpKeepAlive;
  private final MethodHandle httpView;
  private final MethodHandle httpRespond;
  private final MethodHandle httpRelease;
  private final MethodHandle httpSpans;
  private final MethodHandle httpHeadRelease;
  private final MethodHandle httpRetain;
  private final MethodHandle httpFree;
  private final MethodHandle httpDrain;
  private final MethodHandle httpRetire;
  private final MethodHandle httpSegmentReleaseRetired;
  private final MethodHandle httpSend;
  private final MethodHandle httpPrepare;
  private final MethodHandle httpSegmentAck;
  private final MethodHandle httpSegmentRelease;
  private final MethodHandle httpChunkPrepare;
  private final MethodHandle httpChunkSend;

  private final MethodHandle socketOption;
  private final MethodHandle socketShutdown;

  private final MethodHandle tlsClient;
  private final MethodHandle tlsServer;
  private final MethodHandle tlsContextRelease;
  private final MethodHandle tlsNew;
  private final MethodHandle tlsFeed;
  private final MethodHandle tlsStep;
  private final MethodHandle tlsProtocol;
  private final MethodHandle tlsRelease;
  private final SymbolLookup symbols;
  private volatile @Nullable EngineBindings engine;

  @Override
  public long servingNew(int contexts) {
    try {
      return (long) servingNew.invokeExact(contexts);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long servingSplitNew(int contexts) {
    try {
      return (long) servingSplitNew.invokeExact(contexts);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingWorkers(long application, int context) {
    try {
      return (int) servingWorkers.invokeExact(application, context);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long servingWorkerDriver(
      long workload, long application, int context, int worker, int backend, int limit) {
    try {
      return (long)
          servingWorkerDriver.invokeExact(workload, application, context, worker, backend, limit);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingAvailableCores() {
    try {
      return (int) servingAvailableCores.invokeExact();
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingContextEnter(long application, int replica) {
    try {
      return (int) servingContextEnter.invokeExact(application, replica);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingContextLeave() {
    try {
      return (int) servingContextLeave.invokeExact();
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long servingShardDriver(
      long workload, long application, int replica, int backend, int limit) {
    try {
      return (long) servingDriver.invokeExact(workload, application, replica, backend, limit, 1);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long servingDriver(long workload, long application, int replica, int backend, int limit) {
    try {
      // HotSpot lacks the SVM thread-start hook that resets inherited compiler-worker affinity.
      return (long) servingDriver.invokeExact(workload, application, replica, backend, limit, 0);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long servingSplitDriver(
      long workload, long application, int replica, int backend, int limit) {
    try {
      return (long) servingDriver.invokeExact(workload, application, replica, backend, limit, 2);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long servingListen(
      long workload, long driver, long endpoint, long signature, int backlog) {
    try {
      return (long) servingListen.invokeExact(workload, driver, endpoint, signature, backlog);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingReady(long driver) {
    try {
      return (int) servingReady.invokeExact(driver);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingCpu(long driver) {
    try {
      return (int) servingCpu.invokeExact(driver);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingListenerClose(long driver, long listener) {
    try {
      return (int) servingListenerClose.invokeExact(driver, listener);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int servingClose(long application) {
    try {
      return (int) servingClose.invokeExact(application);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  /**
   * Loads the shared library from the matching native classifier JAR or explicit system property.
   */
  public FfmTransportNative() {
    this(dev.elide.bemo.ffm.NativeLibraryLoader.libraryPath());
  }

  public FfmTransportNative(Path library) {
    if (ValueLayout.ADDRESS.byteSize() != 8)
      throw new UnsupportedOperationException("64-bit JVM required");
    SymbolLookup symbols = SymbolLookup.libraryLookup(library, Arena.global());
    version = bind(symbols, "abi_version", ValueLayout.JAVA_INT);
    if (version() != TransportNative.ABI_VERSION)
      throw new IllegalArgumentException("Unsupported transport ABI version");
    lastError = bind(symbols, "last_error", ValueLayout.JAVA_INT);
    tlsClient =
        bind(
            symbols,
            "tls_client",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    tlsServer =
        bind(
            symbols,
            "tls_server",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    tlsContextRelease =
        bind(symbols, "tls_context_release", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    tlsNew =
        bind(
            symbols,
            "tls_new",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    tlsFeed =
        bind(
            symbols,
            "tls_feed",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    tlsStep =
        bind(
            symbols,
            "tls_step",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    tlsProtocol =
        bind(
            symbols,
            "tls_protocol",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    tlsRelease = bind(symbols, "tls_release", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    this.symbols = symbols;

    ownerNew = bind(symbols, "owner_new", ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG);
    ownerUsed = bind(symbols, "owner_used", ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG);
    ownerRelease = bind(symbols, "owner_release", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    workloadClose = bind(symbols, "workload_close", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    gzipNew =
        bind(
            symbols,
            "gzip_new",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    gzipCompress =
        bind(
            symbols,
            "gzip_compress",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    gzipRelease = bind(symbols, "gzip_release", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    bufferNew =
        bind(
            symbols,
            "buffer_new",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    bufferView =
        bind(
            symbols,
            "buffer_view",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.ADDRESS);
    bufferFreeze =
        bind(
            symbols,
            "buffer_freeze",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    bufferSlice =
        bind(
            symbols,
            "buffer_slice",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    bufferRelease = bind(symbols, "buffer_release", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    driverNew =
        bind(
            symbols,
            "driver_new",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT);
    driverBackend = bind(symbols, "driver_backend", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    driverWake = bind(symbols, "driver_wake", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    driverRelease = bind(symbols, "driver_release", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    driverFallback =
        symbols.find("elide_transport_driver_fallback").isEmpty()
            ? null
            : bind(
                symbols,
                "driver_fallback",
                ValueLayout.JAVA_INT,
                ValueLayout.JAVA_LONG,
                ValueLayout.JAVA_LONG);

    servingNew = bind(symbols, "serving_new", ValueLayout.JAVA_LONG, ValueLayout.JAVA_INT);
    servingSplitNew =
        bind(symbols, "serving_split_new", ValueLayout.JAVA_LONG, ValueLayout.JAVA_INT);
    servingWorkers =
        bind(
            symbols,
            "serving_workers",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    servingWorkerDriver =
        bind(
            symbols,
            "serving_worker_driver",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT);
    servingAvailableCores = bind(symbols, "serving_available_cores", ValueLayout.JAVA_INT);
    servingContextEnter =
        bind(
            symbols,
            "serving_context_enter",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    servingContextLeave = bind(symbols, "serving_context_leave", ValueLayout.JAVA_INT);
    servingDriver =
        bind(
            symbols,
            "serving_driver",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT);
    servingListen =
        bind(
            symbols,
            "serving_listen",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    servingReady = bind(symbols, "serving_ready", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    servingCpu = bind(symbols, "serving_cpu", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    servingListenerClose =
        bind(
            symbols,
            "serving_listener_close",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    servingClose = bind(symbols, "serving_close", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    socketOption =
        bind(
            symbols,
            "socket_option",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT);
    socketShutdown =
        bind(
            symbols,
            "socket_shutdown",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    socketListen =
        bind(
            symbols,
            "socket_listen",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT);
    socketConnect =
        bind(
            symbols,
            "socket_connect",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    socketAccept =
        bind(
            symbols,
            "socket_accept",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    socketAdopt =
        bind(
            symbols,
            "socket_adopt",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    socketDiscard = bind(symbols, "socket_discard", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    socketAddress =
        bind(
            symbols,
            "socket_address",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG);
    socketReceive =
        bind(
            symbols,
            "socket_receive",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    socketReceiveNew =
        bind(
            symbols,
            "socket_receive_new",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    socketReceiveNewResult =
        symbols
            .find("elide_transport_socket_receive_new_result")
            .map(
                symbol ->
                    Linker.nativeLinker()
                        .downcallHandle(
                            symbol,
                            FunctionDescriptor.of(
                                ValueLayout.JAVA_INT,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.ADDRESS)))
            .orElse(null);
    socketSend =
        bind(
            symbols,
            "socket_send",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    socketSendInline =
        symbols
            .find("elide_transport_socket_send_inline")
            .map(
                symbol ->
                    Linker.nativeLinker()
                        .downcallHandle(
                            symbol,
                            FunctionDescriptor.of(
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.ADDRESS,
                                ValueLayout.JAVA_LONG)))
            .orElse(null);
    socketSendGathered =
        symbols
            .find("elide_transport_socket_send_gathered")
            .map(
                symbol ->
                    Linker.nativeLinker()
                        .downcallHandle(
                            symbol,
                            FunctionDescriptor.of(
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.JAVA_LONG,
                                ValueLayout.ADDRESS,
                                ValueLayout.JAVA_INT)))
            .orElse(null);
    socketClose =
        bind(
            symbols,
            "socket_close",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    driverPoll =
        bind(
            symbols,
            "driver_poll",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    socketHttp =
        bind(
            symbols,
            "socket_http",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    socketHttpTls =
        bind(
            symbols,
            "socket_http_tls",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpMethod = bind(symbols, "http_method", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpVersion = bind(symbols, "http_version", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpHeaderCount =
        bind(symbols, "http_header_count", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpKeepAlive = bind(symbols, "http_keep_alive", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpView =
        bind(
            symbols,
            "http_view",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG);
    httpRespond =
        bind(
            symbols,
            "http_respond",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    httpRelease =
        bind(
            symbols,
            "http_release",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpSpans =
        bind(
            symbols,
            "http_spans",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    httpHeadRelease =
        bind(symbols, "http_head_release", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpRetain =
        bind(
            symbols,
            "http_retain",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpDrain = bind(symbols, "http_drain", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpRetire = bind(symbols, "http_retire", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpSegmentReleaseRetired =
        bind(symbols, "http_segment_release_retired", ValueLayout.JAVA_INT, ValueLayout.JAVA_LONG);
    httpFree =
        bind(
            symbols,
            "http_free",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpPrepare =
        bind(
            symbols,
            "http_prepare",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpSend =
        bind(
            symbols,
            "http_send",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
    httpSegmentAck =
        bind(
            symbols,
            "http_segment_ack",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpSegmentRelease =
        bind(
            symbols,
            "http_segment_release",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpChunkPrepare =
        bind(
            symbols,
            "http_chunk_prepare",
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG);
    httpChunkSend =
        bind(
            symbols,
            "http_chunk_send",
            ValueLayout.JAVA_INT,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_LONG,
            ValueLayout.JAVA_INT);
  }

  private static MethodHandle bind(
      SymbolLookup symbols, String name, ValueLayout result, ValueLayout... args) {
    return Linker.nativeLinker()
        .downcallHandle(
            symbols.find("elide_transport_" + name).orElseThrow(),
            FunctionDescriptor.of(result, args));
  }

  @Override
  public int lastError() {
    try {
      return (int) lastError.invokeExact();
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int version() {
    try {
      return (int) version.invokeExact();
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long ownerNew(long limit) {
    try {
      return (long) ownerNew.invokeExact(limit);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long ownerUsed(long owner) {
    try {
      return (long) ownerUsed.invokeExact(owner);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int ownerRelease(long owner) {
    try {
      return (int) ownerRelease.invokeExact(owner);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int workloadClose(long workload) {
    try {
      return (int) workloadClose.invokeExact(workload);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long gzipNew(long workload, int level) {
    try {
      return (long) gzipNew.invokeExact(workload, level);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long gzipCompress(long workload, long encoder, long input) {
    try {
      return (long) gzipCompress.invokeExact(workload, encoder, input);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int gzipRelease(long encoder) {
    try {
      return (int) gzipRelease.invokeExact(encoder);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long bufferNew(long owner, long capacity) {
    try {
      return (long) bufferNew.invokeExact(owner, capacity);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public ByteBuffer bufferView(long buffer) {
    try {
      MemorySegment descriptor = BUFFER_DESCRIPTOR.get();
      int status = (int) bufferView.invokeExact(buffer, descriptor);
      if (status != 0) throw new IllegalArgumentException("Invalid native buffer handle");
      long capacity = descriptor.get(ValueLayout.JAVA_LONG, 8);
      if (capacity > Integer.MAX_VALUE)
        throw new IllegalArgumentException("Buffer exceeds ByteBuffer capacity");
      ByteBuffer view = descriptor.get(ValueLayout.ADDRESS, 0).reinterpret(capacity).asByteBuffer();
      return descriptor.get(ValueLayout.JAVA_LONG, 24) == 0 ? view : view.asReadOnlyBuffer();
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int bufferCapacity(long buffer) {
    try {
      MemorySegment descriptor = BUFFER_DESCRIPTOR.get();
      int status = (int) bufferView.invokeExact(buffer, descriptor);
      if (status != 0) throw new IllegalArgumentException("Invalid native buffer handle");
      long capacity = descriptor.get(ValueLayout.JAVA_LONG, 8);
      if (capacity > Integer.MAX_VALUE)
        throw new IllegalArgumentException("Buffer exceeds ByteBuffer capacity");
      return (int) capacity;
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int bufferFreeze(long buffer, long length) {
    try {
      return (int) bufferFreeze.invokeExact(buffer, length);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long bufferSlice(long buffer, long offset, long length) {
    try {
      return (long) bufferSlice.invokeExact(buffer, offset, length);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int bufferRelease(long buffer) {
    try {
      return (int) bufferRelease.invokeExact(buffer);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long driverNew(long workload, int backend, int limit) {
    try {
      return (long) driverNew.invokeExact(workload, backend, limit);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int driverBackend(long driver) {
    try {
      return (int) driverBackend.invokeExact(driver);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int driverWake(long driver) {
    try {
      return (int) driverWake.invokeExact(driver);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int driverRelease(long driver) {
    try {
      return (int) driverRelease.invokeExact(driver);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int driverFallback(long driver, long output) {
    if (driverFallback == null) return TransportNative.super.driverFallback(driver, output);
    try {
      return (int) driverFallback.invokeExact(driver, output);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long socketListen(long workload, long driver, long endpoint, int backlog, int reuse) {
    try {
      return (long) socketListen.invokeExact(workload, driver, endpoint, backlog, reuse);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long socketConnect(long workload, long driver, long endpoint) {
    try {
      return (long) socketConnect.invokeExact(workload, driver, endpoint);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long socketAccept(long workload, long driver, long listener) {
    try {
      return (long) socketAccept.invokeExact(workload, driver, listener);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketAdopt(long workload, long driver, long socket) {
    try {
      return (int) socketAdopt.invokeExact(workload, driver, socket);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketDiscard(long socket) {
    try {
      return (int) socketDiscard.invokeExact(socket);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketAddress(long driver, long socket, int peer, long output) {
    try {
      return (int) socketAddress.invokeExact(driver, socket, peer, output);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long socketReceive(long workload, long driver, long socket, long buffer) {
    try {
      return (long) socketReceive.invokeExact(workload, driver, socket, buffer);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long socketReceiveNew(long workload, long driver, long socket, long owner, long capacity) {
    try {
      return (long) socketReceiveNew.invokeExact(workload, driver, socket, owner, capacity);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public boolean supportsReceiveResults() {
    return socketReceiveNewResult != null;
  }

  @Override
  public ReceiveResult socketReceiveNewResult(
      long workload, long driver, long socket, long owner, long capacity) {
    MethodHandle receive = socketReceiveNewResult;
    if (receive == null)
      throw new UnsupportedOperationException("Immediate native receive results");
    MemorySegment output = BUFFER_DESCRIPTOR.get();
    final int status;
    try {
      status = (int) receive.invokeExact(workload, driver, socket, owner, capacity, output);
    } catch (Throwable error) {
      throw failure(error);
    }
    if (status < 0)
      throw new IllegalStateException("Native receive allocation or admission failed");
    long operation = output.get(ValueLayout.JAVA_LONG, 0);
    long buffer = output.get(ValueLayout.JAVA_LONG, 8);
    long result = output.get(ValueLayout.JAVA_LONG, 24);
    try {
      ByteBuffer bytes =
          result > 0
              ? output.get(ValueLayout.ADDRESS, 16).reinterpret(result).asByteBuffer()
              : null;
      return new ReceiveResult(operation, buffer, result, bytes);
    } catch (RuntimeException | Error error) {
      if (buffer != 0) bufferRelease(buffer);
      throw error;
    }
  }

  @Override
  public long socketSend(
      long workload, long driver, long socket, long buffer, long offset, long length) {
    try {
      return (long) socketSend.invokeExact(workload, driver, socket, buffer, offset, length);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public boolean supportsInlineWrites() {
    return socketSendInline != null;
  }

  @Override
  public long socketSendInline(long workload, long driver, long socket, ByteBuffer source) {
    MethodHandle send = socketSendInline;
    if (send == null) throw new UnsupportedOperationException("Inline native writes");
    if (!source.isDirect() || !source.hasRemaining() || source.remaining() > 128 * 1024) return -1;
    MemorySegment bytes = MemorySegment.ofBuffer(source);
    long length = source.remaining();
    try {
      return (long) send.invokeExact(workload, driver, socket, bytes, length);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public boolean supportsGatheredWrites() {
    return socketSendGathered != null;
  }

  @Override
  public long socketSendGathered(
      long workload, long driver, long socket, long[] regions, int count) {
    if (count < 1 || count > 64 || regions.length < count * 3) return 0;
    MethodHandle send = socketSendGathered;
    if (send == null) throw new UnsupportedOperationException("Gathered native writes");
    MemorySegment descriptors = SEND_REGIONS.get();
    MemorySegment.copy(MemorySegment.ofArray(regions), 0, descriptors, 0, count * 24L);
    try {
      return (long) send.invokeExact(workload, driver, socket, descriptors, count);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketClose(long driver, long socket) {
    try {
      return (int) socketClose.invokeExact(driver, socket);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int driverPoll(long driver, long timeoutNanos, long batch, int maximum) {
    try {
      return (int) driverPoll.invokeExact(driver, timeoutNanos, batch, maximum);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketOption(long driver, long socket, int option, int value) {
    try {
      return (int) socketOption.invokeExact(driver, socket, option, value);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketShutdown(long driver, long socket, int direction) {
    try {
      return (int) socketShutdown.invokeExact(driver, socket, direction);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  private static RuntimeException failure(Throwable error) {
    if (error instanceof RuntimeException runtime) return runtime;
    if (error instanceof Error fatal) throw fatal;
    return new AssertionFailure(error);
  }

  private static final class AssertionFailure extends RuntimeException {
    private static final long serialVersionUID = 1L;

    AssertionFailure(Throwable cause) {
      super("Native ABI invocation failed", cause, false, false);
    }
  }

  @Override
  public long tlsClient(long workload, long roots, long alpn) {
    try {
      return (long) tlsClient.invokeExact(workload, roots, alpn);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long tlsServer(long workload, long chain, long key, long alpn) {
    try {
      return (long) tlsServer.invokeExact(workload, chain, key, alpn);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int tlsContextRelease(long context) {
    try {
      return (int) tlsContextRelease.invokeExact(context);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long tlsNew(long workload, long context, long owner, long name) {
    try {
      return (long) tlsNew.invokeExact(workload, context, owner, name);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int tlsFeed(long session, long input, long length) {
    try {
      return (int) tlsFeed.invokeExact(session, input, length);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int tlsStep(
      long session, int action, long plaintext, long offset, long length, long output) {
    try {
      return (int) tlsStep.invokeExact(session, action, plaintext, offset, length, output);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int tlsProtocol(long session, long output) {
    try {
      return (int) tlsProtocol.invokeExact(session, output);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int tlsRelease(long session) {
    try {
      return (int) tlsRelease.invokeExact(session);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketHttp(long workload, long driver, long socket, long owner, long capacity) {
    try {
      return (int) socketHttp.invokeExact(workload, driver, socket, owner, capacity);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int socketHttpTls(
      long workload, long driver, long socket, long owner, long capacity, long context) {
    try {
      return (int) socketHttpTls.invokeExact(workload, driver, socket, owner, capacity, context);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpMethod(long exchange) {
    try {
      return (int) httpMethod.invokeExact(exchange);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpVersion(long exchange) {
    try {
      return (int) httpVersion.invokeExact(exchange);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpHeaderCount(long exchange) {
    try {
      return (int) httpHeaderCount.invokeExact(exchange);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpKeepAlive(long exchange) {
    try {
      return (int) httpKeepAlive.invokeExact(exchange);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpView(long exchange, int kind, int index, long output) {
    try {
      return (int) httpView.invokeExact(exchange, kind, index, output);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpRespond(
      long driver,
      long exchange,
      int status,
      long headers,
      int count,
      long body,
      long bodyLength,
      int flags) {
    try {
      return (int)
          httpRespond.invokeExact(
              driver, exchange, status, headers, count, body, bodyLength, flags);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpRelease(long driver, long exchange) {
    try {
      return (int) httpRelease.invokeExact(driver, exchange);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long bufferAddress(long buffer) {
    ByteBuffer view = bufferView(buffer);
    return view == null ? 0 : MemorySegment.ofBuffer(view).address();
  }

  @Override
  public byte[] bytes(long address, int length) {
    if (length == 0) return new byte[0];
    return MemorySegment.ofAddress(address).reinterpret(length).toArray(ValueLayout.JAVA_BYTE);
  }

  @Override
  public int httpSpans(long exchange, long output, int capacity) {
    try {
      return (int) httpSpans.invokeExact(exchange, output, capacity);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long httpRetain(long exchange, long lengthOut) {
    try {
      return (long) httpRetain.invokeExact(exchange, lengthOut);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpRetire(long driver) {
    try {
      return (int) httpRetire.invokeExact(driver);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpSegmentReleaseRetired(long segment) {
    try {
      return (int) httpSegmentReleaseRetired.invokeExact(segment);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpDrain(long driver) {
    try {
      return (int) httpDrain.invokeExact(driver);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpFree(long driver, long exchange) {
    try {
      return (int) httpFree.invokeExact(driver, exchange);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpHeadRelease(long head) {
    try {
      return (int) httpHeadRelease.invokeExact(head);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long httpPrepare(long exchange, long capacity) {
    try {
      return (long) httpPrepare.invokeExact(exchange, capacity);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpSend(long driver, long exchange, long length, int flags) {
    try {
      return (int) httpSend.invokeExact(driver, exchange, length, flags);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpSegmentAck(long driver, long segment) {
    try {
      return (int) httpSegmentAck.invokeExact(driver, segment);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpSegmentRelease(long driver, long segment) {
    try {
      return (int) httpSegmentRelease.invokeExact(driver, segment);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long httpChunkPrepare(long exchange, long capacity, long addressOut) {
    try {
      return (long) httpChunkPrepare.invokeExact(exchange, capacity, addressOut);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int httpChunkSend(long driver, long exchange, long buffer, long length, int flags) {
    // CHUNK_RETAIN ownership is enforced by the shared ABI; forward flags unchanged.
    try {
      return (int) httpChunkSend.invokeExact(driver, exchange, buffer, length, flags);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public void copyBytes(long address, byte[] target, int offset, int length) {
    MemorySegment.copy(ALL, ValueLayout.JAVA_BYTE, address, target, offset, length);
  }

  @Override
  public void writeBytes(long address, byte[] source, int offset, int length) {
    MemorySegment.copy(source, offset, ALL, ValueLayout.JAVA_BYTE, address, length);
  }

  @Override
  public ByteBuffer memory(long address, int length) {
    return MemorySegment.ofAddress(address).reinterpret(length).asByteBuffer();
  }

  @Override
  public long getLong(long address) {
    return ALL.get(ValueLayout.JAVA_LONG, address);
  }

  @Override
  public int getInt(long address) {
    return ALL.get(ValueLayout.JAVA_INT, address);
  }

  @Override
  public byte getByte(long address) {
    return ALL.get(ValueLayout.JAVA_BYTE, address);
  }

  // ---- SSLEngine (begin) ----------------------------------------------------------------------

  /** Engine downcalls; heap variants are critical so the native side reads arrays in place. */
  private record EngineBindings(
      MethodHandle contextNew,
      MethodHandle contextRelease,
      MethodHandle engineNew,
      MethodHandle wrap,
      MethodHandle wrapHeap,
      MethodHandle unwrap,
      MethodHandle unwrapHeap,
      MethodHandle control,
      MethodHandle info,
      MethodHandle release) {
    static @Nullable EngineBindings bind(SymbolLookup symbols) {
      if (symbols.find("elide_transport_engine_new").isEmpty()) return null;
      ValueLayout l = ValueLayout.JAVA_LONG;
      ValueLayout i = ValueLayout.JAVA_INT;
      ValueLayout a = ValueLayout.ADDRESS;
      return new EngineBindings(
          FfmTransportNative.bind(symbols, "engine_context_new", l, l, i, l, l, l, l, l, l),
          FfmTransportNative.bind(symbols, "engine_context_release", i, l),
          FfmTransportNative.bind(symbols, "engine_new", l, l, l, l, l),
          FfmTransportNative.bind(symbols, "engine_wrap", l, l, l, l, l, l),
          critical(symbols, "engine_wrap", FunctionDescriptor.of(l, l, a, l, a, l)),
          FfmTransportNative.bind(symbols, "engine_unwrap", l, l, l, l, l, l, i),
          critical(symbols, "engine_unwrap", FunctionDescriptor.of(l, l, a, l, a, l, i)),
          FfmTransportNative.bind(symbols, "engine_control", l, l, i),
          critical(symbols, "engine_info", FunctionDescriptor.of(i, l, i, i, a, l)),
          FfmTransportNative.bind(symbols, "engine_release", i, l));
    }

    private static MethodHandle critical(
        SymbolLookup symbols, String name, FunctionDescriptor descriptor) {
      return Linker.nativeLinker()
          .downcallHandle(
              symbols.find("elide_transport_" + name).orElseThrow(),
              descriptor,
              Linker.Option.critical(true));
    }
  }

  /** Bound on first use, so processes that never create an engine pay no lookup. */
  private EngineBindings engine() {
    EngineBindings bindings = engine;
    if (bindings == null) {
      synchronized (this) {
        bindings = engine;
        if (bindings == null) engine = bindings = EngineBindings.bind(symbols);
      }
      if (bindings == null)
        throw new UnsupportedOperationException("Native TLS engines are unavailable");
    }
    return bindings;
  }

  private static MemorySegment heapSegment(@Nullable ByteBuffer buffer, int length) {
    return length == 0
        ? MemorySegment.NULL
        : MemorySegment.ofBuffer(java.util.Objects.requireNonNull(buffer));
  }

  private static long directAddress(@Nullable ByteBuffer buffer, int length) {
    return length == 0
        ? 0
        : MemorySegment.ofBuffer(java.util.Objects.requireNonNull(buffer)).address();
  }

  private static boolean direct(@Nullable ByteBuffer buffer, int length) {
    return length == 0 || java.util.Objects.requireNonNull(buffer).isDirect();
  }

  private static MemorySegment copy(Arena arena, byte @Nullable [] bytes) {
    return bytes == null || bytes.length == 0
        ? MemorySegment.NULL
        : arena.allocate(bytes.length).copyFrom(MemorySegment.ofArray(bytes));
  }

  @Override
  public boolean supportsEngine() {
    return symbols.find("elide_transport_engine_new").isPresent();
  }

  @Override
  public long engineContextNew(
      long workload,
      int flags,
      byte @Nullable [] certificates,
      byte @Nullable [] key,
      byte @Nullable [] alpn) {
    try (Arena arena = Arena.ofConfined()) {
      MemorySegment chain = copy(arena, certificates);
      MemorySegment secret = copy(arena, key);
      MemorySegment protocols = copy(arena, alpn);
      try {
        return (long)
            engine()
                .contextNew()
                .invokeExact(
                    workload,
                    flags,
                    chain.address(),
                    chain.byteSize(),
                    secret.address(),
                    secret.byteSize(),
                    protocols.address(),
                    protocols.byteSize());
      } finally {
        secret.fill((byte) 0);
      }
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int engineContextRelease(long context) {
    try {
      return (int) engine().contextRelease().invokeExact(context);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long engineNew(long workload, long context, byte @Nullable [] name) {
    try (Arena arena = Arena.ofConfined()) {
      MemorySegment bytes = copy(arena, name);
      return (long)
          engine().engineNew().invokeExact(workload, context, bytes.address(), bytes.byteSize());
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long engineWrap(
      long engine,
      @Nullable ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    try {
      EngineBindings bindings = engine();
      if (direct(source, sourceLength) && direct(destination, destinationLength))
        return (long)
            bindings
                .wrap()
                .invokeExact(
                    engine,
                    directAddress(source, sourceLength),
                    (long) sourceLength,
                    directAddress(destination, destinationLength),
                    (long) destinationLength);
      return (long)
          bindings
              .wrapHeap()
              .invokeExact(
                  engine,
                  heapSegment(source, sourceLength),
                  (long) sourceLength,
                  heapSegment(destination, destinationLength),
                  (long) destinationLength);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long engineUnwrap(
      long engine,
      ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    int flags = source.isReadOnly() ? 1 : 0;
    try {
      EngineBindings bindings = engine();
      if (direct(source, sourceLength) && direct(destination, destinationLength))
        return (long)
            bindings
                .unwrap()
                .invokeExact(
                    engine,
                    directAddress(source, sourceLength),
                    (long) sourceLength,
                    directAddress(destination, destinationLength),
                    (long) destinationLength,
                    flags);
      return (long)
          bindings
              .unwrapHeap()
              .invokeExact(
                  engine,
                  heapSegment(source, sourceLength),
                  (long) sourceLength,
                  heapSegment(destination, destinationLength),
                  (long) destinationLength,
                  flags);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public long engineControl(long engine, int operation) {
    try {
      return (long) engine().control().invokeExact(engine, operation);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int engineInfo(long engine, int kind, int index, byte @Nullable [] output) {
    try {
      MemorySegment target =
          output == null || output.length == 0 ? MemorySegment.NULL : MemorySegment.ofArray(output);
      return (int) engine().info().invokeExact(engine, kind, index, target, target.byteSize());
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  @Override
  public int engineRelease(long engine) {
    try {
      return (int) engine().release().invokeExact(engine);
    } catch (Throwable error) {
      throw failure(error);
    }
  }

  // ---- SSLEngine (end) ------------------------------------------------------------------------
}
