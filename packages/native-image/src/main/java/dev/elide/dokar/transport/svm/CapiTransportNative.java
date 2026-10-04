/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.dokar.transport.svm;

import dev.elide.dokar.transport.TransportNative;
import java.lang.foreign.MemorySegment;
import java.nio.ByteBuffer;
import java.util.List;
import org.graalvm.nativeimage.CurrentIsolate;
import org.graalvm.nativeimage.IsolateThread;
import org.graalvm.nativeimage.PinnedObject;
import org.graalvm.nativeimage.StackValue;
import org.graalvm.nativeimage.c.CContext;
import org.graalvm.nativeimage.c.function.CEntryPoint;
import org.graalvm.nativeimage.c.function.CEntryPointLiteral;
import org.graalvm.nativeimage.c.function.CFunction;
import org.graalvm.nativeimage.c.function.CFunctionPointer;
import org.graalvm.nativeimage.c.function.CLibrary;
import org.graalvm.nativeimage.c.function.InvokeCFunctionPointer;
import org.graalvm.nativeimage.c.struct.CField;
import org.graalvm.nativeimage.c.struct.CStruct;
import org.graalvm.nativeimage.c.type.CTypeConversion;
import org.graalvm.nativeimage.c.type.VoidPointer;
import org.graalvm.word.Pointer;
import org.graalvm.word.PointerBase;
import org.graalvm.word.WordFactory;
import org.jspecify.annotations.Nullable;

/** Direct Native Image bindings; no FFM lookup or JNI transition on this path. */
@CContext(CapiTransportNative.Headers.class)
@CLibrary(value = "dokar_ffi", requireStatic = true)
public final class CapiTransportNative implements TransportNative {

  @CFunction("elide_transport_serving_helper_start")
  public static native int servingHelperStart(long token);

  @CFunction(
      value = "elide_transport_serving_helper_prepare",
      transition = CFunction.Transition.NO_TRANSITION)
  public static native long servingHelperPrepare();

  @CFunction(
      value = "elide_transport_serving_helper_release",
      transition = CFunction.Transition.NO_TRANSITION)
  public static native int servingHelperRelease(long token);

  @CFunction("elide_transport_serving_available_cores")
  private static native int servingAvailableCores0();

  @Override
  public int servingAvailableCores() {
    return servingAvailableCores0();
  }

  @CFunction("elide_transport_serving_context_enter")
  private static native int servingContextEnter0(long application, int replica);

  @Override
  public int servingContextEnter(long application, int replica) {
    return servingContextEnter0(application, replica);
  }

  @CFunction("elide_transport_serving_context_leave")
  private static native int servingContextLeave0();

  @Override
  public int servingContextLeave() {
    return servingContextLeave0();
  }

  @Override
  public long servingShardDriver(
      long workload, long application, int replica, int backend, int limit) {
    return servingDriver0(workload, application, replica, backend, limit, 1);
  }

  @CFunction("elide_transport_serving_new")
  private static native long servingNew0(int contexts);

  @CFunction("elide_transport_serving_split_new")
  private static native long servingSplitNew0(int contexts);

  @Override
  public long servingSplitNew(int contexts) {
    return servingSplitNew0(contexts);
  }

  @CFunction("elide_transport_serving_workers")
  private static native int servingWorkers0(long application, int context);

  @Override
  public int servingWorkers(long application, int context) {
    return servingWorkers0(application, context);
  }

  @CFunction("elide_transport_serving_worker_driver")
  private static native long servingWorkerDriver0(
      long workload, long application, int context, int worker, int backend, int limit);

  @Override
  public long servingWorkerDriver(
      long workload, long application, int context, int worker, int backend, int limit) {
    return servingWorkerDriver0(workload, application, context, worker, backend, limit);
  }

  @Override
  public long servingNew(int contexts) {
    return servingNew0(contexts);
  }

  @CFunction("elide_transport_serving_driver")
  private static native long servingDriver0(
      long workload, long application, int replica, int backend, int limit, int affinity);

  @Override
  public long servingDriver(long workload, long application, int replica, int backend, int limit) {
    return servingDriver0(workload, application, replica, backend, limit, 1);
  }

  @Override
  public long servingSplitDriver(
      long workload, long application, int replica, int backend, int limit) {
    return servingDriver0(workload, application, replica, backend, limit, 2);
  }

  @CFunction("elide_transport_serving_listen")
  private static native long servingListen0(
      long workload, long driver, long endpoint, long signature, int backlog);

  @Override
  public long servingListen(
      long workload, long driver, long endpoint, long signature, int backlog) {
    return servingListen0(workload, driver, endpoint, signature, backlog);
  }

  @CFunction("elide_transport_serving_ready")
  private static native int servingReady0(long driver);

  @Override
  public int servingReady(long driver) {
    return servingReady0(driver);
  }

  @CFunction("elide_transport_serving_cpu")
  private static native int servingCpu0(long driver);

  @Override
  public int servingCpu(long driver) {
    return servingCpu0(driver);
  }

  @CFunction("elide_transport_serving_listener_close")
  private static native int servingListenerClose0(long driver, long listener);

  @Override
  public int servingListenerClose(long driver, long listener) {
    return servingListenerClose0(driver, listener);
  }

  @CFunction("elide_transport_serving_close")
  private static native int servingClose0(long application);

  @Override
  public int servingClose(long application) {
    return servingClose0(application);
  }

  public static final class Headers implements CContext.Directives {
    @Override
    public List<String> getHeaderFiles() {
      return List.of("<elide_transport.h>");
    }
  }

  @CStruct("elide_transport_buffer_view_t")
  interface View extends PointerBase {
    @CField("address")
    VoidPointer address();

    @CField("capacity")
    long capacity();

    @CField("flags")
    long flags();
  }

  @CFunction("elide_transport_buffer_view")
  private static native int view(long buffer, View output);

  @CFunction("elide_transport_last_error")
  private static native int lastError0();

  @Override
  public int lastError() {
    return lastError0();
  }

  @CFunction("elide_transport_abi_version")
  private static native int version0();

  @Override
  public int version() {
    return version0();
  }

  @CFunction("elide_transport_owner_new")
  private static native long ownerNew0(long limit);

  @Override
  public long ownerNew(long limit) {
    return ownerNew0(limit);
  }

  @CFunction("elide_transport_owner_used")
  private static native long ownerUsed0(long owner);

  @Override
  public long ownerUsed(long owner) {
    return ownerUsed0(owner);
  }

  @CFunction("elide_transport_owner_release")
  private static native int ownerRelease0(long owner);

  @Override
  public int ownerRelease(long owner) {
    return ownerRelease0(owner);
  }

  @CFunction("elide_transport_workload_close")
  private static native int workloadClose0(long workload);

  @Override
  public int workloadClose(long workload) {
    return workloadClose0(workload);
  }

  @CFunction("elide_transport_buffer_new")
  private static native long bufferNew0(long owner, long capacity);

  @Override
  public long bufferNew(long owner, long capacity) {
    return bufferNew0(owner, capacity);
  }

  @CFunction("elide_transport_buffer_freeze")
  private static native int bufferFreeze0(long buffer, long length);

  @Override
  public int bufferFreeze(long buffer, long length) {
    return bufferFreeze0(buffer, length);
  }

  @CFunction("elide_transport_buffer_slice")
  private static native long bufferSlice0(long buffer, long offset, long length);

  @Override
  public long bufferSlice(long buffer, long offset, long length) {
    return bufferSlice0(buffer, offset, length);
  }

  @CFunction("elide_transport_buffer_release")
  private static native int bufferRelease0(long buffer);

  @Override
  public int bufferRelease(long buffer) {
    return bufferRelease0(buffer);
  }

  @CFunction("elide_transport_driver_new")
  private static native long driverNew0(long workload, int backend, int limit);

  @Override
  public long driverNew(long workload, int backend, int limit) {
    return driverNew0(workload, backend, limit);
  }

  @CFunction("elide_transport_driver_backend")
  private static native int driverBackend0(long driver);

  @Override
  public int driverBackend(long driver) {
    return driverBackend0(driver);
  }

  @CFunction("elide_transport_driver_wake")
  private static native int driverWake0(long driver);

  @Override
  public int driverWake(long driver) {
    return driverWake0(driver);
  }

  @CFunction("elide_transport_driver_release")
  private static native int driverRelease0(long driver);

  @Override
  public int driverRelease(long driver) {
    return driverRelease0(driver);
  }

  @CFunction("elide_transport_driver_fallback")
  private static native int driverFallback0(long driver, long output);

  @Override
  public int driverFallback(long driver, long output) {
    return driverFallback0(driver, output);
  }

  @CFunction("elide_transport_socket_listen")
  private static native long socketListen0(
      long workload, long driver, long endpoint, int backlog, int reuse);

  @Override
  public long socketListen(long workload, long driver, long endpoint, int backlog, int reuse) {
    return socketListen0(workload, driver, endpoint, backlog, reuse);
  }

  @CFunction("elide_transport_socket_connect")
  private static native long socketConnect0(long workload, long driver, long endpoint);

  @Override
  public long socketConnect(long workload, long driver, long endpoint) {
    return socketConnect0(workload, driver, endpoint);
  }

  @CFunction("elide_transport_socket_accept")
  private static native long socketAccept0(long workload, long driver, long listener);

  @Override
  public long socketAccept(long workload, long driver, long listener) {
    return socketAccept0(workload, driver, listener);
  }

  @CFunction("elide_transport_socket_adopt")
  private static native int socketAdopt0(long workload, long driver, long socket);

  @Override
  public int socketAdopt(long workload, long driver, long socket) {
    return socketAdopt0(workload, driver, socket);
  }

  @CFunction("elide_transport_socket_discard")
  private static native int socketDiscard0(long socket);

  @Override
  public int socketDiscard(long socket) {
    return socketDiscard0(socket);
  }

  @CFunction("elide_transport_socket_address")
  private static native int socketAddress0(long driver, long socket, int peer, long output);

  @Override
  public int socketAddress(long driver, long socket, int peer, long output) {
    return socketAddress0(driver, socket, peer, output);
  }

  @CFunction("elide_transport_socket_receive")
  private static native long socketReceive0(long workload, long driver, long socket, long buffer);

  @Override
  public long socketReceive(long workload, long driver, long socket, long buffer) {
    return socketReceive0(workload, driver, socket, buffer);
  }

  @CFunction("elide_transport_socket_receive_new")
  private static native long socketReceiveNew0(
      long workload, long driver, long socket, long owner, long capacity);

  @Override
  public long socketReceiveNew(long workload, long driver, long socket, long owner, long capacity) {
    return socketReceiveNew0(workload, driver, socket, owner, capacity);
  }

  @CFunction("elide_transport_socket_send")
  private static native long socketSend0(
      long workload, long driver, long socket, long buffer, long offset, long length);

  @Override
  public long socketSend(
      long workload, long driver, long socket, long buffer, long offset, long length) {
    return socketSend0(workload, driver, socket, buffer, offset, length);
  }

  @CFunction("elide_transport_socket_close")
  private static native int socketClose0(long driver, long socket);

  @Override
  public int socketClose(long driver, long socket) {
    return socketClose0(driver, socket);
  }

  @CFunction("elide_transport_driver_poll")
  private static native int driverPoll0(long driver, long timeoutNanos, long batch, int maximum);

  @Override
  public int driverPoll(long driver, long timeoutNanos, long batch, int maximum) {
    return driverPoll0(driver, timeoutNanos, batch, maximum);
  }

  private interface PollCallback extends CFunctionPointer {
    @InvokeCFunctionPointer
    int invoke(IsolateThread thread, Pointer events, int count);
  }

  private static final CEntryPointLiteral<PollCallback> POLL_CALLBACK =
      CEntryPointLiteral.create(
          CapiTransportNative.class, "pollEvent", IsolateThread.class, Pointer.class, int.class);

  private static final class PollState {
    @Nullable EventCallback callback;
    @Nullable Throwable failure;
  }

  private static final ThreadLocal<PollState> POLL_STATE = ThreadLocal.withInitial(PollState::new);

  @CEntryPoint
  private static int pollEvent(IsolateThread thread, Pointer events, int count) {
    PollState state = POLL_STATE.get();
    int consumed = 0;
    try {
      while (consumed < count) {
        Pointer event = events.add(consumed * 40);
        consumed++;
        if (!java.util.Objects.requireNonNull(
                state.callback, "poll callback is installed before entering native code")
            .event(
                event.readLong(0),
                event.readLong(8),
                event.readLong(16),
                event.readLong(24),
                event.readInt(32))) return -consumed;
      }
      return consumed;
    } catch (Throwable failure) {
      state.failure = failure;
      return -consumed;
    }
  }

  @CFunction("elide_transport_driver_poll_batch_callback")
  private static native int driverPollCallback0(
      long workload,
      long driver,
      long timeoutNanos,
      int maximum,
      PollCallback callback,
      IsolateThread thread);

  @Override
  public boolean supportsPollCallback() {
    return true;
  }

  @Override
  public int driverPollCallback(
      long workload, long driver, long timeoutNanos, int maximum, EventCallback callback) {
    PollState state = POLL_STATE.get();
    if (state.callback != null) throw new IllegalStateException("Recursive native callback poll");
    state.callback = java.util.Objects.requireNonNull(callback);
    try {
      int count =
          driverPollCallback0(
              workload,
              driver,
              timeoutNanos,
              maximum,
              POLL_CALLBACK.getFunctionPointer(),
              CurrentIsolate.getCurrentThread());
      if (state.failure instanceof Error error) throw error;
      if (state.failure instanceof RuntimeException failure) throw failure;
      if (state.failure != null) throw new AssertionError(state.failure);
      return count;
    } finally {
      state.callback = null;
      state.failure = null;
    }
  }

  @Override
  public long bufferAddress(long buffer) {
    View output = StackValue.get(View.class);
    if (view(buffer, output) != 0) return 0;
    return output.address().rawValue();
  }

  @Override
  public void copyBytes(long address, byte[] target, int offset, int length) {
    if (offset < 0 || length < 0 || offset > target.length - length) throw invalidByteRange();
    if (length > 0) {
      CTypeConversion.asByteBuffer(WordFactory.pointer(address), length)
          .get(target, offset, length);
    }
  }

  @Override
  public void writeBytes(long address, byte[] source, int offset, int length) {
    if (offset < 0 || length < 0 || offset > source.length - length) throw invalidByteRange();
    if (length > 0) {
      CTypeConversion.asByteBuffer(WordFactory.pointer(address), length)
          .put(source, offset, length);
    }
  }

  private static IndexOutOfBoundsException invalidByteRange() {
    return new IndexOutOfBoundsException("Invalid native byte copy range");
  }

  @Override
  public byte[] bytes(long address, int length) {
    byte[] out = new byte[length];
    if (length > 0) {
      copyBytes(address, out, 0, length);
    }
    return out;
  }

  @CFunction("elide_transport_socket_http")
  private static native int socketHttp0(
      long workload, long driver, long socket, long owner, long capacity);

  @Override
  public int socketHttp(long workload, long driver, long socket, long owner, long capacity) {
    return socketHttp0(workload, driver, socket, owner, capacity);
  }

  @CFunction("elide_transport_socket_http_tls")
  private static native int socketHttpTls0(
      long workload, long driver, long socket, long owner, long capacity, long context);

  @Override
  public int socketHttpTls(
      long workload, long driver, long socket, long owner, long capacity, long context) {
    return socketHttpTls0(workload, driver, socket, owner, capacity, context);
  }

  @CFunction(value = "elide_transport_http_method", transition = CFunction.Transition.NO_TRANSITION)
  private static native int httpMethod0(long exchange);

  @Override
  public int httpMethod(long exchange) {
    return httpMethod0(exchange);
  }

  @CFunction(
      value = "elide_transport_http_version",
      transition = CFunction.Transition.NO_TRANSITION)
  private static native int httpVersion0(long exchange);

  @Override
  public int httpVersion(long exchange) {
    return httpVersion0(exchange);
  }

  @CFunction(
      value = "elide_transport_http_header_count",
      transition = CFunction.Transition.NO_TRANSITION)
  private static native int httpHeaderCount0(long exchange);

  @Override
  public int httpHeaderCount(long exchange) {
    return httpHeaderCount0(exchange);
  }

  @CFunction(
      value = "elide_transport_http_keep_alive",
      transition = CFunction.Transition.NO_TRANSITION)
  private static native int httpKeepAlive0(long exchange);

  @Override
  public int httpKeepAlive(long exchange) {
    return httpKeepAlive0(exchange);
  }

  @CFunction(value = "elide_transport_http_view", transition = CFunction.Transition.NO_TRANSITION)
  private static native int httpView0(long exchange, int kind, int index, long output);

  @Override
  public int httpView(long exchange, int kind, int index, long output) {
    return httpView0(exchange, kind, index, output);
  }

  @CFunction("elide_transport_http_respond")
  private static native int httpRespond0(
      long driver,
      long exchange,
      int status,
      long headers,
      int count,
      long body,
      long bodyLength,
      int flags);

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
    return httpRespond0(driver, exchange, status, headers, count, body, bodyLength, flags);
  }

  @CFunction(value = "elide_transport_http_spans", transition = CFunction.Transition.NO_TRANSITION)
  private static native int httpSpans0(long exchange, long output, int capacity);

  @Override
  public int httpSpans(long exchange, long output, int capacity) {
    return httpSpans0(exchange, output, capacity);
  }

  @CFunction("elide_transport_http_release")
  private static native int httpRelease0(long driver, long exchange);

  @Override
  public int httpRelease(long driver, long exchange) {
    return httpRelease0(driver, exchange);
  }

  @CFunction("elide_transport_http_retain")
  private static native long httpRetain0(long exchange, long lengthOut);

  @Override
  public long httpRetain(long exchange, long lengthOut) {
    return httpRetain0(exchange, lengthOut);
  }

  @CFunction("elide_transport_http_retire")
  private static native int httpRetire0(long driver);

  @Override
  public int httpRetire(long driver) {
    return httpRetire0(driver);
  }

  @CFunction("elide_transport_http_segment_release_retired")
  private static native int httpSegmentReleaseRetired0(long segment);

  @Override
  public int httpSegmentReleaseRetired(long segment) {
    return httpSegmentReleaseRetired0(segment);
  }

  @CFunction("elide_transport_http_drain")
  private static native int httpDrain0(long driver);

  @Override
  public int httpDrain(long driver) {
    return httpDrain0(driver);
  }

  @CFunction("elide_transport_http_free")
  private static native int httpFree0(long driver, long exchange);

  @Override
  public int httpFree(long driver, long exchange) {
    return httpFree0(driver, exchange);
  }

  @CFunction("elide_transport_http_head_release")
  private static native int httpHeadRelease0(long head);

  @Override
  public int httpHeadRelease(long head) {
    return httpHeadRelease0(head);
  }

  @CFunction("elide_transport_http_prepare")
  private static native long httpPrepare0(long exchange, long capacity);

  @Override
  public long httpPrepare(long exchange, long capacity) {
    return httpPrepare0(exchange, capacity);
  }

  @CFunction("elide_transport_http_send")
  private static native int httpSend0(long driver, long exchange, long length, int flags);

  @Override
  public int httpSend(long driver, long exchange, long length, int flags) {
    return httpSend0(driver, exchange, length, flags);
  }

  // Default transition: mutates driver state and may submit a receive, so it must allow
  // safepoints.
  @CFunction("elide_transport_http_segment_ack")
  private static native int httpSegmentAck0(long driver, long segment);

  @Override
  public int httpSegmentAck(long driver, long segment) {
    return httpSegmentAck0(driver, segment);
  }

  // Default transition: this allocates, frees, or mutates driver state, so it must allow
  // safepoints.
  @CFunction("elide_transport_http_segment_release")
  private static native int httpSegmentRelease0(long driver, long segment);

  @Override
  public int httpSegmentRelease(long driver, long segment) {
    return httpSegmentRelease0(driver, segment);
  }

  // Default transition: allocates native storage.
  @CFunction("elide_transport_http_chunk_prepare")
  private static native long httpChunkPrepare0(long exchange, long capacity, long addressOut);

  @Override
  public long httpChunkPrepare(long exchange, long capacity, long addressOut) {
    return httpChunkPrepare0(exchange, capacity, addressOut);
  }

  // Default transition: mutates driver state and may free the buffer.
  @CFunction("elide_transport_http_chunk_send")
  private static native int httpChunkSend0(
      long driver, long exchange, long buffer, long length, int flags);

  @Override
  public int httpChunkSend(long driver, long exchange, long buffer, long length, int flags) {
    return httpChunkSend0(driver, exchange, buffer, length, flags);
  }

  @Override
  public ByteBuffer memory(long address, int length) {
    return CTypeConversion.asByteBuffer(WordFactory.pointer(address), length);
  }

  @Override
  public long getLong(long address) {
    Pointer pointer = WordFactory.pointer(address);
    return pointer.readLong(0);
  }

  @Override
  public int getInt(long address) {
    Pointer pointer = WordFactory.pointer(address);
    return pointer.readInt(0);
  }

  @Override
  public byte getByte(long address) {
    Pointer pointer = WordFactory.pointer(address);
    return pointer.readByte(0);
  }

  @CFunction("elide_transport_socket_option")
  private static native int socketOption0(long driver, long socket, int option, int value);

  @Override
  public int socketOption(long driver, long socket, int option, int value) {
    return socketOption0(driver, socket, option, value);
  }

  @CFunction("elide_transport_socket_shutdown")
  private static native int socketShutdown0(long driver, long socket, int direction);

  @Override
  public int socketShutdown(long driver, long socket, int direction) {
    return socketShutdown0(driver, socket, direction);
  }

  @Override
  public ByteBuffer bufferView(long buffer) {
    View output = StackValue.get(View.class);
    if (view(buffer, output) != 0)
      throw new IllegalArgumentException("Invalid native buffer handle");
    long capacity = output.capacity();
    if (capacity > Integer.MAX_VALUE)
      throw new IllegalArgumentException("Buffer exceeds ByteBuffer capacity");
    ByteBuffer view = CTypeConversion.asByteBuffer(output.address(), (int) capacity);
    return output.flags() == 0 ? view : view.asReadOnlyBuffer();
  }

  @Override
  public int bufferCapacity(long buffer) {
    View output = StackValue.get(View.class);
    if (view(buffer, output) != 0)
      throw new IllegalArgumentException("Invalid native buffer handle");
    long capacity = output.capacity();
    if (capacity > Integer.MAX_VALUE)
      throw new IllegalArgumentException("Buffer exceeds ByteBuffer capacity");
    return (int) capacity;
  }

  @CFunction("elide_transport_tls_client")
  private static native long tlsClient0(long workload, long roots, long alpn);

  @Override
  public long tlsClient(long workload, long roots, long alpn) {
    return tlsClient0(workload, roots, alpn);
  }

  @CFunction("elide_transport_tls_server")
  private static native long tlsServer0(long workload, long chain, long key, long alpn);

  @Override
  public long tlsServer(long workload, long chain, long key, long alpn) {
    return tlsServer0(workload, chain, key, alpn);
  }

  @CFunction("elide_transport_tls_context_release")
  private static native int tlsContextRelease0(long context);

  @Override
  public int tlsContextRelease(long context) {
    return tlsContextRelease0(context);
  }

  @CFunction("elide_transport_tls_new")
  private static native long tlsNew0(long workload, long context, long owner, long name);

  @Override
  public long tlsNew(long workload, long context, long owner, long name) {
    return tlsNew0(workload, context, owner, name);
  }

  @CFunction("elide_transport_tls_feed")
  private static native int tlsFeed0(long session, long input, long length);

  @Override
  public int tlsFeed(long session, long input, long length) {
    return tlsFeed0(session, input, length);
  }

  @CFunction("elide_transport_tls_step")
  private static native int tlsStep0(
      long session, int action, long plaintext, long offset, long length, long output);

  @Override
  public int tlsStep(
      long session, int action, long plaintext, long offset, long length, long output) {
    return tlsStep0(session, action, plaintext, offset, length, output);
  }

  @CFunction("elide_transport_tls_protocol")
  private static native int tlsProtocol0(long session, long output);

  @Override
  public int tlsProtocol(long session, long output) {
    return tlsProtocol0(session, output);
  }

  @CFunction("elide_transport_tls_release")
  private static native int tlsRelease0(long session);

  @Override
  public int tlsRelease(long session) {
    return tlsRelease0(session);
  }

  // ---- SSLEngine (begin) ----------------------------------------------------------------------

  @CFunction("elide_transport_engine_context_new")
  private static native long engineContextNew0(
      long workload,
      int flags,
      long certificates,
      long certificatesLength,
      long key,
      long keyLength,
      long alpn,
      long alpnLength);

  @CFunction("elide_transport_engine_context_release")
  private static native int engineContextRelease0(long context);

  @CFunction("elide_transport_engine_new")
  private static native long engineNew0(long workload, long context, long name, long nameLength);

  @CFunction("elide_transport_engine_wrap")
  private static native long engineWrap0(
      long engine, long source, long sourceLength, long destination, long destinationLength);

  @CFunction("elide_transport_engine_unwrap")
  private static native long engineUnwrap0(
      long engine,
      long source,
      long sourceLength,
      long destination,
      long destinationLength,
      int flags);

  @CFunction("elide_transport_engine_control")
  private static native long engineControl0(long engine, int operation);

  @CFunction("elide_transport_engine_info")
  private static native int engineInfo0(
      long engine, int kind, int index, long output, long capacity);

  @CFunction("elide_transport_engine_release")
  private static native int engineRelease0(long engine);

  /** Heap arrays stay pinned for the call; direct buffers pass their position's address. */
  // address() applies arrayOffset and position after pinning the entire backing array.
  @SuppressWarnings("ByteBufferBackingArray")
  private static @Nullable PinnedObject pin(@Nullable ByteBuffer buffer, int length) {
    return length == 0 || java.util.Objects.requireNonNull(buffer).isDirect()
        ? null
        : PinnedObject.create(buffer.array());
  }

  private static long address(
      @Nullable ByteBuffer buffer, int length, @Nullable PinnedObject pinned) {
    if (length == 0) return 0;
    if (pinned == null)
      return MemorySegment.ofBuffer(java.util.Objects.requireNonNull(buffer)).address();
    return pinned
        .addressOfArrayElement(
            java.util.Objects.requireNonNull(buffer).arrayOffset() + buffer.position())
        .rawValue();
  }

  private static @Nullable PinnedObject pin(byte @Nullable [] bytes) {
    return bytes == null || bytes.length == 0 ? null : PinnedObject.create(bytes);
  }

  private static long address(@Nullable PinnedObject pinned) {
    return pinned == null ? 0 : pinned.addressOfArrayElement(0).rawValue();
  }

  private static void unpin(@Nullable PinnedObject pinned) {
    if (pinned != null) pinned.close();
  }

  @Override
  public boolean supportsEngine() {
    return true;
  }

  @Override
  public long engineContextNew(
      long workload,
      int flags,
      byte @Nullable [] certificates,
      byte @Nullable [] key,
      byte @Nullable [] alpn) {
    PinnedObject chain = pin(certificates), secret = pin(key), protocols = pin(alpn);
    try {
      return engineContextNew0(
          workload,
          flags,
          address(chain),
          certificates == null ? 0 : certificates.length,
          address(secret),
          key == null ? 0 : key.length,
          address(protocols),
          alpn == null ? 0 : alpn.length);
    } finally {
      unpin(chain);
      unpin(secret);
      unpin(protocols);
    }
  }

  @Override
  public int engineContextRelease(long context) {
    return engineContextRelease0(context);
  }

  @Override
  public long engineNew(long workload, long context, byte @Nullable [] name) {
    @Nullable PinnedObject pinned = pin(name);
    try {
      return engineNew0(workload, context, address(pinned), name == null ? 0 : name.length);
    } finally {
      unpin(pinned);
    }
  }

  @Override
  public long engineWrap(
      long engine,
      @Nullable ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    PinnedObject input = pin(source, sourceLength), output = pin(destination, destinationLength);
    try {
      return engineWrap0(
          engine,
          address(source, sourceLength, input),
          sourceLength,
          address(destination, destinationLength, output),
          destinationLength);
    } finally {
      unpin(input);
      unpin(output);
    }
  }

  @Override
  public long engineUnwrap(
      long engine,
      ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    PinnedObject input = pin(source, sourceLength), output = pin(destination, destinationLength);
    try {
      return engineUnwrap0(
          engine,
          address(source, sourceLength, input),
          sourceLength,
          address(destination, destinationLength, output),
          destinationLength,
          source.isReadOnly() ? 1 : 0);
    } finally {
      unpin(input);
      unpin(output);
    }
  }

  @Override
  public long engineControl(long engine, int operation) {
    return engineControl0(engine, operation);
  }

  @Override
  public int engineInfo(long engine, int kind, int index, byte @Nullable [] output) {
    @Nullable PinnedObject pinned = pin(output);
    try {
      return engineInfo0(engine, kind, index, address(pinned), output == null ? 0 : output.length);
    } finally {
      unpin(pinned);
    }
  }

  @Override
  public int engineRelease(long engine) {
    return engineRelease0(engine);
  }

  // ---- SSLEngine (end) ------------------------------------------------------------------------
}
