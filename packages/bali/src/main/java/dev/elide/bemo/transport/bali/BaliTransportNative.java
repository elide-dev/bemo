/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport.bali;

import bali.api.BemoNative;
import dev.elide.bemo.transport.TransportNative;
import java.nio.ByteBuffer;

/** Direct image calls; the guest retains its own Bemo and framework classes. */
public final class BaliTransportNative implements TransportNative {
  private final boolean completeHttp1 =
      (BemoNative.bemo_capabilities() & CAP_HTTP1_COMPLETE_CREDIT) != 0;

  @Override
  public boolean supportsCompleteHttp1Responses() {
    return completeHttp1;
  }

  private static int servingAvailableCores0() {
    return BemoNative.elide_transport_serving_available_cores();
  }

  @Override
  public int servingAvailableCores() {
    return servingAvailableCores0();
  }

  private static int servingContextEnter0(long application, int replica) {
    return BemoNative.elide_transport_serving_context_enter(application, replica);
  }

  @Override
  public int servingContextEnter(long application, int replica) {
    return servingContextEnter0(application, replica);
  }

  private static int servingContextLeave0() {
    return BemoNative.elide_transport_serving_context_leave();
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
    return BemoNative.elide_transport_serving_new(contexts);
  }

  private static long servingSplitNew0(int contexts) {
    return BemoNative.elide_transport_serving_split_new(contexts);
  }

  @Override
  public long servingSplitNew(int contexts) {
    return servingSplitNew0(contexts);
  }

  private static int servingWorkers0(long application, int context) {
    return BemoNative.elide_transport_serving_workers(application, context);
  }

  @Override
  public int servingWorkers(long application, int context) {
    return servingWorkers0(application, context);
  }

  private static long servingWorkerDriver0(
      long workload, long application, int context, int worker, int backend, int limit) {
    return BemoNative.elide_transport_serving_worker_driver(
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
    return BemoNative.elide_transport_serving_driver(
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
    return BemoNative.elide_transport_serving_listen(
        workload, driver, endpoint, signature, backlog);
  }

  @Override
  public long servingListen(
      long workload, long driver, long endpoint, long signature, int backlog) {
    return servingListen0(workload, driver, endpoint, signature, backlog);
  }

  private static int servingReady0(long driver) {
    return BemoNative.elide_transport_serving_ready(driver);
  }

  @Override
  public int servingReady(long driver) {
    return servingReady0(driver);
  }

  private static int servingCpu0(long driver) {
    return BemoNative.elide_transport_serving_cpu(driver);
  }

  @Override
  public int servingCpu(long driver) {
    return servingCpu0(driver);
  }

  private static int servingListenerClose0(long driver, long listener) {
    return BemoNative.elide_transport_serving_listener_close(driver, listener);
  }

  @Override
  public int servingListenerClose(long driver, long listener) {
    return servingListenerClose0(driver, listener);
  }

  private static int servingClose0(long application) {
    return BemoNative.elide_transport_serving_close(application);
  }

  @Override
  public int servingClose(long application) {
    return servingClose0(application);
  }

  private static int lastError0() {
    return BemoNative.elide_transport_last_error();
  }

  @Override
  public int lastError() {
    return lastError0();
  }

  private static int version0() {
    return BemoNative.elide_transport_abi_version();
  }

  @Override
  public int version() {
    return version0();
  }

  private static long ownerNew0(long limit) {
    return BemoNative.elide_transport_owner_new(limit);
  }

  @Override
  public long ownerNew(long limit) {
    return ownerNew0(limit);
  }

  private static long ownerUsed0(long owner) {
    return BemoNative.elide_transport_owner_used(owner);
  }

  @Override
  public long ownerUsed(long owner) {
    return ownerUsed0(owner);
  }

  private static int ownerRelease0(long owner) {
    return BemoNative.elide_transport_owner_release(owner);
  }

  @Override
  public int ownerRelease(long owner) {
    return ownerRelease0(owner);
  }

  private static int workloadClose0(long workload) {
    return BemoNative.elide_transport_workload_close(workload);
  }

  @Override
  public int workloadClose(long workload) {
    return workloadClose0(workload);
  }

  private static long bufferNew0(long owner, long capacity) {
    return BemoNative.elide_transport_buffer_new(owner, capacity);
  }

  @Override
  public long bufferNew(long owner, long capacity) {
    return bufferNew0(owner, capacity);
  }

  private static int bufferFreeze0(long buffer, long length) {
    return BemoNative.elide_transport_buffer_freeze(buffer, length);
  }

  @Override
  public int bufferFreeze(long buffer, long length) {
    return bufferFreeze0(buffer, length);
  }

  private static long bufferSlice0(long buffer, long offset, long length) {
    return BemoNative.elide_transport_buffer_slice(buffer, offset, length);
  }

  @Override
  public long bufferSlice(long buffer, long offset, long length) {
    return bufferSlice0(buffer, offset, length);
  }

  private static int bufferRelease0(long buffer) {
    return BemoNative.elide_transport_buffer_release(buffer);
  }

  @Override
  public int bufferRelease(long buffer) {
    return bufferRelease0(buffer);
  }

  private static long driverNew0(long workload, int backend, int limit) {
    return BemoNative.elide_transport_driver_new(workload, backend, limit);
  }

  @Override
  public long driverNew(long workload, int backend, int limit) {
    return driverNew0(workload, backend, limit);
  }

  private static int driverBackend0(long driver) {
    return BemoNative.elide_transport_driver_backend(driver);
  }

  @Override
  public int driverBackend(long driver) {
    return driverBackend0(driver);
  }

  private static int driverWake0(long driver) {
    return BemoNative.elide_transport_driver_wake(driver);
  }

  @Override
  public int driverWake(long driver) {
    return driverWake0(driver);
  }

  private static int driverRelease0(long driver) {
    return BemoNative.elide_transport_driver_release(driver);
  }

  @Override
  public int driverRelease(long driver) {
    return driverRelease0(driver);
  }

  private static int driverFallback0(long driver, long output) {
    return BemoNative.elide_transport_driver_fallback(driver, output);
  }

  @Override
  public int driverFallback(long driver, long output) {
    return driverFallback0(driver, output);
  }

  private static long socketListen0(
      long workload, long driver, long endpoint, int backlog, int reuse) {
    return BemoNative.elide_transport_socket_listen(workload, driver, endpoint, backlog, reuse);
  }

  @Override
  public long socketListen(long workload, long driver, long endpoint, int backlog, int reuse) {
    return socketListen0(workload, driver, endpoint, backlog, reuse);
  }

  private static long socketConnect0(long workload, long driver, long endpoint) {
    return BemoNative.elide_transport_socket_connect(workload, driver, endpoint);
  }

  @Override
  public long socketConnect(long workload, long driver, long endpoint) {
    return socketConnect0(workload, driver, endpoint);
  }

  private static long socketAccept0(long workload, long driver, long listener) {
    return BemoNative.elide_transport_socket_accept(workload, driver, listener);
  }

  @Override
  public long socketAccept(long workload, long driver, long listener) {
    return socketAccept0(workload, driver, listener);
  }

  private static int socketAdopt0(long workload, long driver, long socket) {
    return BemoNative.elide_transport_socket_adopt(workload, driver, socket);
  }

  @Override
  public int socketAdopt(long workload, long driver, long socket) {
    return socketAdopt0(workload, driver, socket);
  }

  private static int socketDiscard0(long socket) {
    return BemoNative.elide_transport_socket_discard(socket);
  }

  @Override
  public int socketDiscard(long socket) {
    return socketDiscard0(socket);
  }

  private static int socketAddress0(long driver, long socket, int peer, long output) {
    return BemoNative.elide_transport_socket_address(driver, socket, peer, output);
  }

  @Override
  public int socketAddress(long driver, long socket, int peer, long output) {
    return socketAddress0(driver, socket, peer, output);
  }

  private static long socketReceive0(long workload, long driver, long socket, long buffer) {
    return BemoNative.elide_transport_socket_receive(workload, driver, socket, buffer);
  }

  @Override
  public long socketReceive(long workload, long driver, long socket, long buffer) {
    return socketReceive0(workload, driver, socket, buffer);
  }

  private static long socketReceiveNew0(
      long workload, long driver, long socket, long owner, long capacity) {
    return BemoNative.elide_transport_socket_receive_new(workload, driver, socket, owner, capacity);
  }

  @Override
  public long socketReceiveNew(long workload, long driver, long socket, long owner, long capacity) {
    return socketReceiveNew0(workload, driver, socket, owner, capacity);
  }

  private static long socketSend0(
      long workload, long driver, long socket, long buffer, long offset, long length) {
    return BemoNative.elide_transport_socket_send(workload, driver, socket, buffer, offset, length);
  }

  @Override
  public long socketSend(
      long workload, long driver, long socket, long buffer, long offset, long length) {
    return socketSend0(workload, driver, socket, buffer, offset, length);
  }

  private static int socketClose0(long driver, long socket) {
    return BemoNative.elide_transport_socket_close(driver, socket);
  }

  @Override
  public int socketClose(long driver, long socket) {
    return socketClose0(driver, socket);
  }

  private static int driverPoll0(long driver, long timeoutNanos, long batch, int maximum) {
    return BemoNative.elide_transport_driver_poll(driver, timeoutNanos, batch, maximum);
  }

  @Override
  public int driverPoll(long driver, long timeoutNanos, long batch, int maximum) {
    return driverPoll0(driver, timeoutNanos, batch, maximum);
  }

  private static int socketHttp0(
      long workload, long driver, long socket, long owner, long capacity) {
    return BemoNative.elide_transport_socket_http(workload, driver, socket, owner, capacity);
  }

  @Override
  public int socketHttp(long workload, long driver, long socket, long owner, long capacity) {
    return socketHttp0(workload, driver, socket, owner, capacity);
  }

  private static int socketHttpTls0(
      long workload, long driver, long socket, long owner, long capacity, long context) {
    return BemoNative.elide_transport_socket_http_tls(
        workload, driver, socket, owner, capacity, context);
  }

  @Override
  public int socketHttpTls(
      long workload, long driver, long socket, long owner, long capacity, long context) {
    return socketHttpTls0(workload, driver, socket, owner, capacity, context);
  }

  private static int httpMethod0(long exchange) {
    return BemoNative.elide_transport_http_method(exchange);
  }

  @Override
  public int httpMethod(long exchange) {
    return httpMethod0(exchange);
  }

  private static int httpVersion0(long exchange) {
    return BemoNative.elide_transport_http_version(exchange);
  }

  @Override
  public int httpVersion(long exchange) {
    return httpVersion0(exchange);
  }

  private static int httpHeaderCount0(long exchange) {
    return BemoNative.elide_transport_http_header_count(exchange);
  }

  @Override
  public int httpHeaderCount(long exchange) {
    return httpHeaderCount0(exchange);
  }

  private static int httpKeepAlive0(long exchange) {
    return BemoNative.elide_transport_http_keep_alive(exchange);
  }

  @Override
  public int httpKeepAlive(long exchange) {
    return httpKeepAlive0(exchange);
  }

  private static int httpView0(long exchange, int kind, int index, long output) {
    return BemoNative.elide_transport_http_view(exchange, kind, index, output);
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
    return BemoNative.elide_transport_http_respond(
        driver, exchange, status, headers, count, body, bodyLength, flags);
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
    return BemoNative.elide_transport_http_spans(exchange, output, capacity);
  }

  @Override
  public int httpSpans(long exchange, long output, int capacity) {
    return httpSpans0(exchange, output, capacity);
  }

  private static int httpRelease0(long driver, long exchange) {
    return BemoNative.elide_transport_http_release(driver, exchange);
  }

  @Override
  public int httpRelease(long driver, long exchange) {
    return httpRelease0(driver, exchange);
  }

  private static long httpRetain0(long exchange, long lengthOut) {
    return BemoNative.elide_transport_http_retain(exchange, lengthOut);
  }

  @Override
  public long httpRetain(long exchange, long lengthOut) {
    return httpRetain0(exchange, lengthOut);
  }

  private static int httpRetire0(long driver) {
    return BemoNative.elide_transport_http_retire(driver);
  }

  @Override
  public int httpRetire(long driver) {
    return httpRetire0(driver);
  }

  private static int httpSegmentReleaseRetired0(long segment) {
    return BemoNative.elide_transport_http_segment_release_retired(segment);
  }

  @Override
  public int httpSegmentReleaseRetired(long segment) {
    return httpSegmentReleaseRetired0(segment);
  }

  private static int httpDrain0(long driver) {
    return BemoNative.elide_transport_http_drain(driver);
  }

  @Override
  public int httpDrain(long driver) {
    return httpDrain0(driver);
  }

  private static int httpFree0(long driver, long exchange) {
    return BemoNative.elide_transport_http_free(driver, exchange);
  }

  @Override
  public int httpFree(long driver, long exchange) {
    return httpFree0(driver, exchange);
  }

  private static int httpHeadRelease0(long head) {
    return BemoNative.elide_transport_http_head_release(head);
  }

  @Override
  public int httpHeadRelease(long head) {
    return httpHeadRelease0(head);
  }

  private static long httpPrepare0(long exchange, long capacity) {
    return BemoNative.elide_transport_http_prepare(exchange, capacity);
  }

  @Override
  public long httpPrepare(long exchange, long capacity) {
    return httpPrepare0(exchange, capacity);
  }

  private static int httpSend0(long driver, long exchange, long length, int flags) {
    return BemoNative.elide_transport_http_send(driver, exchange, length, flags);
  }

  @Override
  public int httpSend(long driver, long exchange, long length, int flags) {
    return httpSend0(driver, exchange, length, flags);
  }

  private static int httpSegmentAck0(long driver, long segment) {
    return BemoNative.elide_transport_http_segment_ack(driver, segment);
  }

  @Override
  public int httpSegmentAck(long driver, long segment) {
    return httpSegmentAck0(driver, segment);
  }

  private static int httpSegmentRelease0(long driver, long segment) {
    return BemoNative.elide_transport_http_segment_release(driver, segment);
  }

  @Override
  public int httpSegmentRelease(long driver, long segment) {
    return httpSegmentRelease0(driver, segment);
  }

  private static long httpChunkPrepare0(long exchange, long capacity, long addressOut) {
    return BemoNative.elide_transport_http_chunk_prepare(exchange, capacity, addressOut);
  }

  @Override
  public long httpChunkPrepare(long exchange, long capacity, long addressOut) {
    return httpChunkPrepare0(exchange, capacity, addressOut);
  }

  private static int httpChunkSend0(
      long driver, long exchange, long buffer, long length, int flags) {
    return BemoNative.elide_transport_http_chunk_send(driver, exchange, buffer, length, flags);
  }

  @Override
  public int httpChunkSend(long driver, long exchange, long buffer, long length, int flags) {
    // CHUNK_RETAIN ownership is enforced by the shared ABI; forward flags unchanged.
    return httpChunkSend0(driver, exchange, buffer, length, flags);
  }

  private static int socketOption0(long driver, long socket, int option, int value) {
    return BemoNative.elide_transport_socket_option(driver, socket, option, value);
  }

  @Override
  public int socketOption(long driver, long socket, int option, int value) {
    return socketOption0(driver, socket, option, value);
  }

  private static int socketShutdown0(long driver, long socket, int direction) {
    return BemoNative.elide_transport_socket_shutdown(driver, socket, direction);
  }

  @Override
  public int socketShutdown(long driver, long socket, int direction) {
    return socketShutdown0(driver, socket, direction);
  }

  private static long tlsClient0(long workload, long roots, long alpn) {
    return BemoNative.elide_transport_tls_client(workload, roots, alpn);
  }

  @Override
  public long tlsClient(long workload, long roots, long alpn) {
    return tlsClient0(workload, roots, alpn);
  }

  private static long tlsServer0(long workload, long chain, long key, long alpn) {
    return BemoNative.elide_transport_tls_server(workload, chain, key, alpn);
  }

  @Override
  public long tlsServer(long workload, long chain, long key, long alpn) {
    return tlsServer0(workload, chain, key, alpn);
  }

  private static int tlsContextRelease0(long context) {
    return BemoNative.elide_transport_tls_context_release(context);
  }

  @Override
  public int tlsContextRelease(long context) {
    return tlsContextRelease0(context);
  }

  private static long tlsNew0(long workload, long context, long owner, long name) {
    return BemoNative.elide_transport_tls_new(workload, context, owner, name);
  }

  @Override
  public long tlsNew(long workload, long context, long owner, long name) {
    return tlsNew0(workload, context, owner, name);
  }

  private static int tlsFeed0(long session, long input, long length) {
    return BemoNative.elide_transport_tls_feed(session, input, length);
  }

  @Override
  public int tlsFeed(long session, long input, long length) {
    return tlsFeed0(session, input, length);
  }

  private static int tlsStep0(
      long session, int action, long plaintext, long offset, long length, long output) {
    return BemoNative.elide_transport_tls_step(session, action, plaintext, offset, length, output);
  }

  @Override
  public int tlsStep(
      long session, int action, long plaintext, long offset, long length, long output) {
    return tlsStep0(session, action, plaintext, offset, length, output);
  }

  private static int tlsProtocol0(long session, long output) {
    return BemoNative.elide_transport_tls_protocol(session, output);
  }

  @Override
  public int tlsProtocol(long session, long output) {
    return tlsProtocol0(session, output);
  }

  private static int tlsRelease0(long session) {
    return BemoNative.elide_transport_tls_release(session);
  }

  @Override
  public int tlsRelease(long session) {
    return tlsRelease0(session);
  }

  private static int engineContextRelease0(long context) {
    return BemoNative.elide_transport_engine_context_release(context);
  }

  private static long engineControl0(long engine, int operation) {
    return BemoNative.elide_transport_engine_control(engine, operation);
  }

  private static int engineRelease0(long engine) {
    return BemoNative.elide_transport_engine_release(engine);
  }

  @Override
  public int engineContextRelease(long context) {
    return engineContextRelease0(context);
  }

  @Override
  public long engineControl(long engine, int operation) {
    return engineControl0(engine, operation);
  }

  @Override
  public int engineRelease(long engine) {
    return engineRelease0(engine);
  }

  @Override
  public ByteBuffer bufferView(long buffer) {
    return BemoNative.bufferView(buffer);
  }

  @Override
  public int bufferCapacity(long buffer) {
    return BemoNative.bufferCapacity(buffer);
  }

  @Override
  public long bufferAddress(long buffer) {
    return BemoNative.bufferAddress(buffer);
  }

  @Override
  public ByteBuffer memory(long address, int length) {
    return BemoNative.directBuffer(address, length);
  }

  @Override
  public long getLong(long address) {
    return BemoNative.getLong(address);
  }

  @Override
  public int getInt(long address) {
    return BemoNative.getInt(address);
  }

  @Override
  public byte getByte(long address) {
    return BemoNative.getByte(address);
  }

  @Override
  public void copyBytes(long address, byte[] target, int offset, int length) {
    BemoNative.copyBytes(address, target, offset, length);
  }

  @Override
  public void writeBytes(long address, byte[] source, int offset, int length) {
    BemoNative.writeBytes(address, source, offset, length);
  }

  @Override
  public byte[] bytes(long address, int length) {
    byte[] result = new byte[length];
    BemoNative.copyBytes(address, result, 0, length);
    return result;
  }
}
