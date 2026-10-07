/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.bemo.transport.svm;

import dev.elide.bemo.svm.generated.BemoNatives;
import dev.elide.bemo.transport.TransportNative;
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
@CLibrary(value = "bemo_ffi", requireStatic = true)
public final class CapiTransportNative implements TransportNative {

  public static int servingHelperStart(long token) {
    return BemoNatives.elide_transport_serving_helper_start(token);
  }

  public static long servingHelperPrepare() {
    return BemoNatives.elide_transport_serving_helper_prepare();
  }

  public static int servingHelperRelease(long token) {
    return BemoNatives.elide_transport_serving_helper_release(token);
  }

  private static int servingAvailableCores0() {
    return BemoNatives.elide_transport_serving_available_cores();
  }

  @Override
  public int servingAvailableCores() {
    return servingAvailableCores0();
  }

  private static int servingContextEnter0(long application, int replica) {
    return BemoNatives.elide_transport_serving_context_enter(application, replica);
  }

  @Override
  public int servingContextEnter(long application, int replica) {
    return servingContextEnter0(application, replica);
  }

  private static int servingContextLeave0() {
    return BemoNatives.elide_transport_serving_context_leave();
  }

  @Override
  public int servingContextLeave() {
    return servingContextLeave0();
  }

  @Override
  public long servingShardDriver(
      long workload, long application, int replica, int backend, int limit) {
    return servingDriver0(workload, application, replica, backend, limit, 1);
  }

  private static long servingNew0(int contexts) {
    return BemoNatives.elide_transport_serving_new(contexts);
  }

  private static long servingSplitNew0(int contexts) {
    return BemoNatives.elide_transport_serving_split_new(contexts);
  }

  @Override
  public long servingSplitNew(int contexts) {
    return servingSplitNew0(contexts);
  }

  private static int servingWorkers0(long application, int context) {
    return BemoNatives.elide_transport_serving_workers(application, context);
  }

  @Override
  public int servingWorkers(long application, int context) {
    return servingWorkers0(application, context);
  }

  private static long servingWorkerDriver0(
      long workload, long application, int context, int worker, int backend, int limit) {
    return BemoNatives.elide_transport_serving_worker_driver(
        workload, application, context, worker, backend, limit);
  }

  @Override
  public long servingWorkerDriver(
      long workload, long application, int context, int worker, int backend, int limit) {
    return servingWorkerDriver0(workload, application, context, worker, backend, limit);
  }

  @Override
  public long servingNew(int contexts) {
    return servingNew0(contexts);
  }

  private static long servingDriver0(
      long workload, long application, int replica, int backend, int limit, int affinity) {
    return BemoNatives.elide_transport_serving_driver(
        workload, application, replica, backend, limit, affinity);
  }

  @Override
  public long servingDriver(long workload, long application, int replica, int backend, int limit) {
    return servingDriver0(workload, application, replica, backend, limit, 1);
  }

  @Override
  public long servingSplitDriver(
      long workload, long application, int replica, int backend, int limit) {
    return servingDriver0(workload, application, replica, backend, limit, 2);
  }

  private static long servingListen0(
      long workload, long driver, long endpoint, long signature, int backlog) {
    return BemoNatives.elide_transport_serving_listen(
        workload, driver, endpoint, signature, backlog);
  }

  @Override
  public long servingListen(
      long workload, long driver, long endpoint, long signature, int backlog) {
    return servingListen0(workload, driver, endpoint, signature, backlog);
  }

  private static int servingReady0(long driver) {
    return BemoNatives.elide_transport_serving_ready(driver);
  }

  @Override
  public int servingReady(long driver) {
    return servingReady0(driver);
  }

  private static int servingCpu0(long driver) {
    return BemoNatives.elide_transport_serving_cpu(driver);
  }

  @Override
  public int servingCpu(long driver) {
    return servingCpu0(driver);
  }

  private static int servingListenerClose0(long driver, long listener) {
    return BemoNatives.elide_transport_serving_listener_close(driver, listener);
  }

  @Override
  public int servingListenerClose(long driver, long listener) {
    return servingListenerClose0(driver, listener);
  }

  private static int servingClose0(long application) {
    return BemoNatives.elide_transport_serving_close(application);
  }

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

  private static int view(long buffer, View output) {
    return BemoNatives.elide_transport_buffer_view(buffer, output);
  }

  private static int lastError0() {
    return BemoNatives.elide_transport_last_error();
  }

  @Override
  public int lastError() {
    return lastError0();
  }

  private static int version0() {
    return BemoNatives.elide_transport_abi_version();
  }

  @Override
  public int version() {
    return version0();
  }

  private static long ownerNew0(long limit) {
    return BemoNatives.elide_transport_owner_new(limit);
  }

  @Override
  public long ownerNew(long limit) {
    return ownerNew0(limit);
  }

  private static long ownerUsed0(long owner) {
    return BemoNatives.elide_transport_owner_used(owner);
  }

  @Override
  public long ownerUsed(long owner) {
    return ownerUsed0(owner);
  }

  private static int ownerRelease0(long owner) {
    return BemoNatives.elide_transport_owner_release(owner);
  }

  @Override
  public int ownerRelease(long owner) {
    return ownerRelease0(owner);
  }

  private static int workloadClose0(long workload) {
    return BemoNatives.elide_transport_workload_close(workload);
  }

  @Override
  public int workloadClose(long workload) {
    return workloadClose0(workload);
  }

  private static long bufferNew0(long owner, long capacity) {
    return BemoNatives.elide_transport_buffer_new(owner, capacity);
  }

  @Override
  public long bufferNew(long owner, long capacity) {
    return bufferNew0(owner, capacity);
  }

  private static int bufferFreeze0(long buffer, long length) {
    return BemoNatives.elide_transport_buffer_freeze(buffer, length);
  }

  @Override
  public int bufferFreeze(long buffer, long length) {
    return bufferFreeze0(buffer, length);
  }

  private static long bufferSlice0(long buffer, long offset, long length) {
    return BemoNatives.elide_transport_buffer_slice(buffer, offset, length);
  }

  @Override
  public long bufferSlice(long buffer, long offset, long length) {
    return bufferSlice0(buffer, offset, length);
  }

  private static int bufferRelease0(long buffer) {
    return BemoNatives.elide_transport_buffer_release(buffer);
  }

  @Override
  public int bufferRelease(long buffer) {
    return bufferRelease0(buffer);
  }

  private static long driverNew0(long workload, int backend, int limit) {
    return BemoNatives.elide_transport_driver_new(workload, backend, limit);
  }

  @Override
  public long driverNew(long workload, int backend, int limit) {
    return driverNew0(workload, backend, limit);
  }

  private static int driverBackend0(long driver) {
    return BemoNatives.elide_transport_driver_backend(driver);
  }

  @Override
  public int driverBackend(long driver) {
    return driverBackend0(driver);
  }

  private static int driverWake0(long driver) {
    return BemoNatives.elide_transport_driver_wake(driver);
  }

  @Override
  public int driverWake(long driver) {
    return driverWake0(driver);
  }

  private static int driverRelease0(long driver) {
    return BemoNatives.elide_transport_driver_release(driver);
  }

  @Override
  public int driverRelease(long driver) {
    return driverRelease0(driver);
  }

  private static int driverFallback0(long driver, long output) {
    return BemoNatives.elide_transport_driver_fallback(driver, output);
  }

  @Override
  public int driverFallback(long driver, long output) {
    return driverFallback0(driver, output);
  }

  private static long socketListen0(
      long workload, long driver, long endpoint, int backlog, int reuse) {
    return BemoNatives.elide_transport_socket_listen(workload, driver, endpoint, backlog, reuse);
  }

  @Override
  public long socketListen(long workload, long driver, long endpoint, int backlog, int reuse) {
    return socketListen0(workload, driver, endpoint, backlog, reuse);
  }

  private static long socketConnect0(long workload, long driver, long endpoint) {
    return BemoNatives.elide_transport_socket_connect(workload, driver, endpoint);
  }

  @Override
  public long socketConnect(long workload, long driver, long endpoint) {
    return socketConnect0(workload, driver, endpoint);
  }

  private static long socketAccept0(long workload, long driver, long listener) {
    return BemoNatives.elide_transport_socket_accept(workload, driver, listener);
  }

  @Override
  public long socketAccept(long workload, long driver, long listener) {
    return socketAccept0(workload, driver, listener);
  }

  private static int socketAdopt0(long workload, long driver, long socket) {
    return BemoNatives.elide_transport_socket_adopt(workload, driver, socket);
  }

  @Override
  public int socketAdopt(long workload, long driver, long socket) {
    return socketAdopt0(workload, driver, socket);
  }

  private static int socketDiscard0(long socket) {
    return BemoNatives.elide_transport_socket_discard(socket);
  }

  @Override
  public int socketDiscard(long socket) {
    return socketDiscard0(socket);
  }

  private static int socketAddress0(long driver, long socket, int peer, long output) {
    return BemoNatives.elide_transport_socket_address(driver, socket, peer, output);
  }

  @Override
  public int socketAddress(long driver, long socket, int peer, long output) {
    return socketAddress0(driver, socket, peer, output);
  }

  private static long socketReceive0(long workload, long driver, long socket, long buffer) {
    return BemoNatives.elide_transport_socket_receive(workload, driver, socket, buffer);
  }

  @Override
  public long socketReceive(long workload, long driver, long socket, long buffer) {
    return socketReceive0(workload, driver, socket, buffer);
  }

  private static long socketReceiveNew0(
      long workload, long driver, long socket, long owner, long capacity) {
    return BemoNatives.elide_transport_socket_receive_new(
        workload, driver, socket, owner, capacity);
  }

  @Override
  public long socketReceiveNew(long workload, long driver, long socket, long owner, long capacity) {
    return socketReceiveNew0(workload, driver, socket, owner, capacity);
  }

  @CStruct("elide_transport_receive_result_t")
  interface ReceiveOutput extends PointerBase {
    @CField("operation")
    long operation();

    @CField("buffer")
    long buffer();

    @CField("address")
    VoidPointer address();

    @CField("result")
    long result();
  }

  private static int socketReceiveNewResult0(
      long workload, long driver, long socket, long owner, long capacity, ReceiveOutput output) {
    return BemoNatives.elide_transport_socket_receive_new_result(
        workload, driver, socket, owner, capacity, output);
  }

  @Override
  public boolean supportsReceiveResults() {
    return true;
  }

  @Override
  public ReceiveResult socketReceiveNewResult(
      long workload, long driver, long socket, long owner, long capacity) {
    ReceiveOutput output = StackValue.get(ReceiveOutput.class);
    int status = socketReceiveNewResult0(workload, driver, socket, owner, capacity, output);
    if (status < 0)
      throw new IllegalStateException("Native receive allocation or admission failed");
    long operation = output.operation();
    long buffer = output.buffer();
    long result = output.result();
    try {
      ByteBuffer bytes =
          result > 0
              ? CTypeConversion.asByteBuffer(output.address(), Math.toIntExact(result))
              : null;
      return new ReceiveResult(operation, buffer, result, bytes);
    } catch (RuntimeException | Error error) {
      if (buffer != 0) bufferRelease(buffer);
      throw error;
    }
  }

  private static long socketSend0(
      long workload, long driver, long socket, long buffer, long offset, long length) {
    return BemoNatives.elide_transport_socket_send(
        workload, driver, socket, buffer, offset, length);
  }

  @Override
  public long socketSend(
      long workload, long driver, long socket, long buffer, long offset, long length) {
    return socketSend0(workload, driver, socket, buffer, offset, length);
  }

  private static long socketSendInline0(
      long workload, long driver, long socket, Pointer source, long length) {
    return BemoNatives.elide_transport_socket_send_inline(
        workload, driver, socket, WordFactory.pointer(source.rawValue()), length);
  }

  @Override
  public boolean supportsInlineWrites() {
    return true;
  }

  @Override
  public long socketSendInline(long workload, long driver, long socket, ByteBuffer source) {
    if (!source.isDirect() || !source.hasRemaining() || source.remaining() > 128 * 1024) return -1;
    return socketSendInline0(
        workload,
        driver,
        socket,
        WordFactory.pointer(MemorySegment.ofBuffer(source).address()),
        source.remaining());
  }

  private static long socketSendGathered0(
      long workload, long driver, long socket, Pointer regions, int count) {
    return BemoNatives.elide_transport_socket_send_gathered(
        workload, driver, socket, WordFactory.pointer(regions.rawValue()), count);
  }

  @Override
  public boolean supportsGatheredWrites() {
    return true;
  }

  @Override
  public long socketSendGathered(
      long workload, long driver, long socket, long[] regions, int count) {
    if (count < 1 || count > 64 || regions.length < count * 3) return 0;
    try (PinnedObject pinned = PinnedObject.create(regions)) {
      return socketSendGathered0(workload, driver, socket, pinned.addressOfArrayElement(0), count);
    }
  }

  private static int socketClose0(long driver, long socket) {
    return BemoNatives.elide_transport_socket_close(driver, socket);
  }

  @Override
  public int socketClose(long driver, long socket) {
    return socketClose0(driver, socket);
  }

  private static int driverPoll0(long driver, long timeoutNanos, long batch, int maximum) {
    return BemoNatives.elide_transport_driver_poll(driver, timeoutNanos, batch, maximum);
  }

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

  private static int socketHttp0(
      long workload, long driver, long socket, long owner, long capacity) {
    return BemoNatives.elide_transport_socket_http(workload, driver, socket, owner, capacity);
  }

  @Override
  public int socketHttp(long workload, long driver, long socket, long owner, long capacity) {
    return socketHttp0(workload, driver, socket, owner, capacity);
  }

  private static int socketHttpTls0(
      long workload, long driver, long socket, long owner, long capacity, long context) {
    return BemoNatives.elide_transport_socket_http_tls(
        workload, driver, socket, owner, capacity, context);
  }

  @Override
  public int socketHttpTls(
      long workload, long driver, long socket, long owner, long capacity, long context) {
    return socketHttpTls0(workload, driver, socket, owner, capacity, context);
  }

  private static int httpMethod0(long exchange) {
    return BemoNatives.elide_transport_http_method(exchange);
  }

  @Override
  public int httpMethod(long exchange) {
    return httpMethod0(exchange);
  }

  private static int httpVersion0(long exchange) {
    return BemoNatives.elide_transport_http_version(exchange);
  }

  @Override
  public int httpVersion(long exchange) {
    return httpVersion0(exchange);
  }

  private static int httpHeaderCount0(long exchange) {
    return BemoNatives.elide_transport_http_header_count(exchange);
  }

  @Override
  public int httpHeaderCount(long exchange) {
    return httpHeaderCount0(exchange);
  }

  private static int httpKeepAlive0(long exchange) {
    return BemoNatives.elide_transport_http_keep_alive(exchange);
  }

  @Override
  public int httpKeepAlive(long exchange) {
    return httpKeepAlive0(exchange);
  }

  private static int httpView0(long exchange, int kind, int index, long output) {
    return BemoNatives.elide_transport_http_view(
        exchange, kind, index, WordFactory.pointer(output));
  }

  @Override
  public int httpView(long exchange, int kind, int index, long output) {
    return httpView0(exchange, kind, index, output);
  }

  private static int httpRespond0(
      long driver,
      long exchange,
      int status,
      long headers,
      int count,
      long body,
      long bodyLength,
      int flags) {
    return BemoNatives.elide_transport_http_respond(
        driver,
        exchange,
        status,
        WordFactory.pointer(headers),
        count,
        WordFactory.pointer(body),
        bodyLength,
        flags);
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
    return httpRespond0(driver, exchange, status, headers, count, body, bodyLength, flags);
  }

  private static int httpSpans0(long exchange, long output, int capacity) {
    return BemoNatives.elide_transport_http_spans(exchange, WordFactory.pointer(output), capacity);
  }

  @Override
  public int httpSpans(long exchange, long output, int capacity) {
    return httpSpans0(exchange, output, capacity);
  }

  private static int httpRelease0(long driver, long exchange) {
    return BemoNatives.elide_transport_http_release(driver, exchange);
  }

  @Override
  public int httpRelease(long driver, long exchange) {
    return httpRelease0(driver, exchange);
  }

  private static long httpRetain0(long exchange, long lengthOut) {
    return BemoNatives.elide_transport_http_retain(exchange, WordFactory.pointer(lengthOut));
  }

  @Override
  public long httpRetain(long exchange, long lengthOut) {
    return httpRetain0(exchange, lengthOut);
  }

  private static int httpRetire0(long driver) {
    return BemoNatives.elide_transport_http_retire(driver);
  }

  @Override
  public int httpRetire(long driver) {
    return httpRetire0(driver);
  }

  private static int httpSegmentReleaseRetired0(long segment) {
    return BemoNatives.elide_transport_http_segment_release_retired(segment);
  }

  @Override
  public int httpSegmentReleaseRetired(long segment) {
    return httpSegmentReleaseRetired0(segment);
  }

  private static int httpDrain0(long driver) {
    return BemoNatives.elide_transport_http_drain(driver);
  }

  @Override
  public int httpDrain(long driver) {
    return httpDrain0(driver);
  }

  private static int httpFree0(long driver, long exchange) {
    return BemoNatives.elide_transport_http_free(driver, exchange);
  }

  @Override
  public int httpFree(long driver, long exchange) {
    return httpFree0(driver, exchange);
  }

  private static int httpHeadRelease0(long head) {
    return BemoNatives.elide_transport_http_head_release(head);
  }

  @Override
  public int httpHeadRelease(long head) {
    return httpHeadRelease0(head);
  }

  private static long httpPrepare0(long exchange, long capacity) {
    return BemoNatives.elide_transport_http_prepare(exchange, capacity);
  }

  @Override
  public long httpPrepare(long exchange, long capacity) {
    return httpPrepare0(exchange, capacity);
  }

  private static int httpSend0(long driver, long exchange, long length, int flags) {
    return BemoNatives.elide_transport_http_send(driver, exchange, length, flags);
  }

  @Override
  public int httpSend(long driver, long exchange, long length, int flags) {
    return httpSend0(driver, exchange, length, flags);
  }

  // Default transition: mutates driver state and may submit a receive, so it must allow
  // safepoints.
  private static int httpSegmentAck0(long driver, long segment) {
    return BemoNatives.elide_transport_http_segment_ack(driver, segment);
  }

  @Override
  public int httpSegmentAck(long driver, long segment) {
    return httpSegmentAck0(driver, segment);
  }

  // Default transition: this allocates, frees, or mutates driver state, so it must allow
  // safepoints.
  private static int httpSegmentRelease0(long driver, long segment) {
    return BemoNatives.elide_transport_http_segment_release(driver, segment);
  }

  @Override
  public int httpSegmentRelease(long driver, long segment) {
    return httpSegmentRelease0(driver, segment);
  }

  // Default transition: allocates native storage.
  private static long httpChunkPrepare0(long exchange, long capacity, long addressOut) {
    return BemoNatives.elide_transport_http_chunk_prepare(
        exchange, capacity, WordFactory.pointer(addressOut));
  }

  @Override
  public long httpChunkPrepare(long exchange, long capacity, long addressOut) {
    return httpChunkPrepare0(exchange, capacity, addressOut);
  }

  // Default transition: mutates driver state and may free the buffer.
  private static int httpChunkSend0(
      long driver, long exchange, long buffer, long length, int flags) {
    return BemoNatives.elide_transport_http_chunk_send(driver, exchange, buffer, length, flags);
  }

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

  private static int socketOption0(long driver, long socket, int option, int value) {
    return BemoNatives.elide_transport_socket_option(driver, socket, option, value);
  }

  @Override
  public int socketOption(long driver, long socket, int option, int value) {
    return socketOption0(driver, socket, option, value);
  }

  private static int socketShutdown0(long driver, long socket, int direction) {
    return BemoNatives.elide_transport_socket_shutdown(driver, socket, direction);
  }

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

  private static long tlsClient0(long workload, long roots, long alpn) {
    return BemoNatives.elide_transport_tls_client(workload, roots, alpn);
  }

  @Override
  public long tlsClient(long workload, long roots, long alpn) {
    return tlsClient0(workload, roots, alpn);
  }

  private static long tlsServer0(long workload, long chain, long key, long alpn) {
    return BemoNatives.elide_transport_tls_server(workload, chain, key, alpn);
  }

  @Override
  public long tlsServer(long workload, long chain, long key, long alpn) {
    return tlsServer0(workload, chain, key, alpn);
  }

  private static int tlsContextRelease0(long context) {
    return BemoNatives.elide_transport_tls_context_release(context);
  }

  @Override
  public int tlsContextRelease(long context) {
    return tlsContextRelease0(context);
  }

  private static long tlsNew0(long workload, long context, long owner, long name) {
    return BemoNatives.elide_transport_tls_new(workload, context, owner, name);
  }

  @Override
  public long tlsNew(long workload, long context, long owner, long name) {
    return tlsNew0(workload, context, owner, name);
  }

  private static int tlsFeed0(long session, long input, long length) {
    return BemoNatives.elide_transport_tls_feed(session, input, length);
  }

  @Override
  public int tlsFeed(long session, long input, long length) {
    return tlsFeed0(session, input, length);
  }

  private static int tlsStep0(
      long session, int action, long plaintext, long offset, long length, long output) {
    return BemoNatives.elide_transport_tls_step(session, action, plaintext, offset, length, output);
  }

  @Override
  public int tlsStep(
      long session, int action, long plaintext, long offset, long length, long output) {
    return tlsStep0(session, action, plaintext, offset, length, output);
  }

  private static int tlsProtocol0(long session, long output) {
    return BemoNatives.elide_transport_tls_protocol(session, output);
  }

  @Override
  public int tlsProtocol(long session, long output) {
    return tlsProtocol0(session, output);
  }

  private static int tlsRelease0(long session) {
    return BemoNatives.elide_transport_tls_release(session);
  }

  @Override
  public int tlsRelease(long session) {
    return tlsRelease0(session);
  }

  // ---- SSLEngine (begin) ----------------------------------------------------------------------

  private static long engineContextNew0(
      long workload,
      int flags,
      long certificates,
      long certificatesLength,
      long key,
      long keyLength,
      long alpn,
      long alpnLength) {
    return BemoNatives.elide_transport_engine_context_new(
        workload,
        flags,
        WordFactory.pointer(certificates),
        certificatesLength,
        WordFactory.pointer(key),
        keyLength,
        WordFactory.pointer(alpn),
        alpnLength);
  }

  private static int engineContextRelease0(long context) {
    return BemoNatives.elide_transport_engine_context_release(context);
  }

  private static long engineNew0(long workload, long context, long name, long nameLength) {
    return BemoNatives.elide_transport_engine_new(
        workload, context, WordFactory.pointer(name), nameLength);
  }

  private static long engineWrap0(
      long engine, long source, long sourceLength, long destination, long destinationLength) {
    return BemoNatives.elide_transport_engine_wrap(
        engine,
        WordFactory.pointer(source),
        sourceLength,
        WordFactory.pointer(destination),
        destinationLength);
  }

  private static long engineUnwrap0(
      long engine,
      long source,
      long sourceLength,
      long destination,
      long destinationLength,
      int flags) {
    return BemoNatives.elide_transport_engine_unwrap(
        engine,
        WordFactory.pointer(source),
        sourceLength,
        WordFactory.pointer(destination),
        destinationLength,
        flags);
  }

  private static long engineControl0(long engine, int operation) {
    return BemoNatives.elide_transport_engine_control(engine, operation);
  }

  private static int engineInfo0(long engine, int kind, int index, long output, long capacity) {
    return BemoNatives.elide_transport_engine_info(
        engine, kind, index, WordFactory.pointer(output), capacity);
  }

  private static int engineRelease0(long engine) {
    return BemoNatives.elide_transport_engine_release(engine);
  }

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
