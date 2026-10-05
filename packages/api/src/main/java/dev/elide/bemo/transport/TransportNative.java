/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.bemo.transport;

import java.nio.ByteBuffer;
import org.jspecify.annotations.Nullable;

/**
 * Low-level native ABI. Handles own storage; ByteBuffer views do not. Callers must stop using all
 * views before releasing a handle, and abandon writable views before freezing it. Driver methods
 * run on the creating thread, except wakeup. Abandon every view before submitting a receive or
 * transferring a receive into TLS; access resumes only for handles returned by completion. Views of
 * poll and TLS result buffers must be quiescent during their native calls. TLS sessions are
 * thread-owned. No JNI or Elide bootstrap is required.
 *
 * <p>Every method that starts work takes a leading {@code workload}: an owner handle from {@link
 * #ownerNew} (see {@link Workload}). Work on an existing socket must name the workload that created
 * it, and closing a workload cancels only its own operations.
 */
public interface TransportNative {
  int ABI_VERSION = 3;

  int version();

  int lastError();

  long ownerNew(long limit);

  long ownerUsed(long owner);

  /** Close the owner's workload and forget its handle. */
  int ownerRelease(long owner);

  /**
   * Reject the workload's allocations and new work and cancel its operations, leaving other
   * workloads on the same drivers untouched. The handle stays valid until {@link #ownerRelease}.
   */
  int workloadClose(long workload);

  long bufferNew(long owner, long capacity);

  ByteBuffer bufferView(long buffer);

  default int bufferCapacity(long buffer) {
    return bufferView(buffer).capacity();
  }

  int bufferFreeze(long buffer, long length);

  long bufferSlice(long buffer, long offset, long length);

  int bufferRelease(long buffer);

  long driverNew(long workload, int backend, int limit);

  int driverBackend(long driver);

  int driverWake(long driver);

  int driverRelease(long driver);

  /**
   * Write why AUTO fell back from io_uring into mutable {@code output}; returns length, 0 if none.
   */
  default int driverFallback(long driver, long output) {
    throw new UnsupportedOperationException("Driver fallback reporting is unavailable");
  }

  /** Native topology handles are control-plane only; each driver is confined to its OS thread. */
  long servingNew(int contexts);

  /** Resolve all transport owners once, with each owner assigned to exactly one context. */
  long servingSplitNew(int contexts);

  int servingWorkers(long application, int context);

  long servingWorkerDriver(
      long workload, long application, int context, int worker, int backend, int limit);

  default int servingAvailableCores() {
    throw new UnsupportedOperationException("Serving topology is unavailable");
  }

  /** Enter only after starting the transport thread, which must inherit the full CPU mask. */
  default int servingContextEnter(long application, int replica) {
    throw new UnsupportedOperationException("Serving context placement is unavailable");
  }

  default int servingContextLeave() {
    throw new UnsupportedOperationException("Serving context placement is unavailable");
  }

  /** Pinned transport owner paired with a guest context on the same physical core. */
  default long servingShardDriver(
      long workload, long application, int replica, int backend, int limit) {
    throw new UnsupportedOperationException("Serving shard placement is unavailable");
  }

  long servingDriver(long workload, long application, int replica, int backend, int limit);

  /** Unpinned context listener; accepted sockets transfer to its dedicated transport workers. */
  default long servingSplitDriver(
      long workload, long application, int replica, int backend, int limit) {
    throw new UnsupportedOperationException("Split serving is unavailable");
  }

  long servingListen(long workload, long driver, long endpoint, long signature, int backlog);

  int servingReady(long driver);

  int servingCpu(long driver);

  int servingListenerClose(long driver, long listener);

  int servingClose(long application);

  long socketListen(long workload, long driver, long endpoint, int backlog, int reuse);

  long socketConnect(long workload, long driver, long endpoint);

  /** Accepted sockets belong to the listener's workload. */
  long socketAccept(long workload, long driver, long listener);

  /** A workload other than the listener's leaves the socket with the caller. */
  int socketAdopt(long workload, long driver, long socket);

  int socketDiscard(long socket);

  int socketAddress(long driver, long socket, int peer, long output);

  long socketReceive(long workload, long driver, long socket, long buffer);

  /**
   * Allocate receive storage charged to {@code owner}; completion publishes its initialized bytes.
   */
  long socketReceiveNew(long workload, long driver, long socket, long owner, long capacity);

  long socketSend(long workload, long driver, long socket, long buffer, long offset, long length);

  int socketClose(long driver, long socket);

  /** A timeout of -1 (native UINT64_MAX) waits until I/O or an explicit wake. */
  int driverPoll(long driver, long timeoutNanos, long batch, int maximum);

  @FunctionalInterface
  interface EventCallback {
    boolean event(long operation, long socket, long value, long result, int kind);
  }

  default boolean supportsPollCallback() {
    return false;
  }

  /**
   * Dispatch on the owning thread within native completion processing; native APIs may be
   * reentered. A closed callback workload is rejected.
   */
  default int driverPollCallback(
      long workload, long driver, long timeoutNanos, int maximum, EventCallback callback) {
    throw new UnsupportedOperationException("Native poll callbacks unavailable");
  }

  int socketOption(long driver, long socket, int option, int value);

  int socketShutdown(long driver, long socket, int direction);

  /**
   * HTTP mode: the driver parses requests (poll event kind {@link #EVENT_REQUEST}) and encodes
   * responses.
   */
  int EVENT_REQUEST = 5;

  /** The driver closed an HTTP-mode socket after its final response or a parse error. */
  int EVENT_CLOSED = 6;

  /**
   * A body segment of a streaming request. Poll event {@code operation} is the segment handle,
   * {@code value} the exchange, {@code result} the segment length.
   */
  int EVENT_BODY = 7;

  /**
   * A pending body ended. Poll event {@code value} is the exchange; {@code result} is zero on a
   * clean end, or a negative portable error when the body was truncated, malformed, or discarded
   * unread.
   */
  int EVENT_BODY_END = 8;

  /**
   * A queued response part of a streaming exchange reached the wire. Poll event {@code value} is
   * the exchange, {@code result} the part's bytes on the wire, or a negative portable error when it
   * was dropped. Emitted once per queued part, the head included, in queue order.
   */
  int EVENT_PART_SENT = 9;

  /**
   * A single HTTP/2 stream was reset; {@code value} is its exchange. Sibling streams remain live.
   */
  int EVENT_RESET = 10;

  /** Shard-local listener closed; socket identifies its local handle. */
  int EVENT_LISTENER_CLOSED = 11;

  /** Native startup phase changed; value is 1 (ready) or 2 (closed). */
  int EVENT_SERVING_PHASE = 12;

  /**
   * Bit 0 of the exchange layout's flags byte (offset 31): the request has a body, delivered as
   * {@link #EVENT_BODY} segments and ending with {@link #EVENT_BODY_END}.
   */
  int EXCHANGE_FLAG_BODY = 1;

  int VIEW_METHOD = 0;
  int VIEW_PATH = 1;
  int VIEW_HEADER_NAME = 2;
  int VIEW_HEADER_VALUE = 3;
  int VIEW_BODY = 4;

  /**
   * Switch an attached socket to HTTP mode; receive storage of {@code capacity} is charged to
   * {@code owner}.
   */
  int socketHttp(long workload, long driver, long socket, long owner, long capacity);

  /** Activate native HTTP/1.1 over a retained server TLS context. */
  int socketHttpTls(
      long workload, long driver, long socket, long owner, long capacity, long context);

  /**
   * Method code of an exchange (0 GET, 1 HEAD, 2 POST, 3 PUT, 4 DELETE, 5 CONNECT, 6 OPTIONS, 7
   * TRACE, 8 PATCH, 255 other).
   */
  int httpMethod(long exchange);

  /** 0 for HTTP/1.0, 1 for HTTP/1.1. */
  int httpVersion(long exchange);

  int httpHeaderCount(long exchange);

  /** 1 when the connection stays open after this exchange, 0 when it closes. */
  int httpKeepAlive(long exchange);

  /**
   * Write the address and length of a request byte range into the 16 bytes at {@code output}. The
   * bytes stay valid until the exchange is responded to or released.
   */
  int httpView(long exchange, int kind, int index, long output);

  /** Flag for {@link #httpRespond}: send the head only; the body follows as parts. */
  int RESPOND_STREAM = 2;

  /**
   * Respond to an exchange; its memory stays readable until {@link #httpFree}. {@code headers}
   * addresses {@code count} records of four longs (name address, name length, value address, value
   * length); memory is read during the call only.
   *
   * <p>With {@link #RESPOND_STREAM} in {@code flags}, only the head is encoded and {@code body} is
   * ignored: {@code bodyLength} declares the body ({@code content-length}), or {@code -1L} for an
   * unknown length ({@code transfer-encoding: chunked}). Parts then follow through {@link
   * #httpChunkSend}.
   */
  int httpRespond(
      long driver,
      long exchange,
      int status,
      long headers,
      int count,
      long body,
      long bodyLength,
      int flags);

  /** Drop an exchange without responding; the connection closes after earlier responses. */
  int httpRelease(long driver, long exchange);

  /**
   * Write the request's byte ranges within the head as {@code int} pairs at {@code output}: method,
   * path, then name and value per header. Returns the slots needed; nothing is written when that
   * exceeds {@code capacity}.
   */
  int httpSpans(long exchange, long output, int capacity);

  /**
   * Copy a live exchange's head into a caller-owned blob; the blob's length is written to the
   * native address {@code lengthOut}. Any thread while the exchange is unfreed; zero on failure.
   */
  long httpRetain(long exchange, long lengthOut);

  /** Release a blob from {@link #httpRetain}; any thread. */
  int httpHeadRelease(long head);

  /**
   * Free an exchange the JVM no longer reads. Abandons it first if it was never responded to.
   * Driver thread only; every request event must reach this exactly once.
   */
  int httpFree(long driver, long exchange);

  /** Stop admitting heads and return HTTP connections still draining their bodies and writes. */
  int httpDrain(long driver);

  /** Close HTTP sockets and preserve delivered body storage before retiring the driver queue. */
  int httpRetire(long driver);

  /** Release retained body storage after retirement, from any thread. */
  int httpSegmentReleaseRetired(long segment);

  /**
   * Allocate the exchange's response buffer of {@code capacity} bytes; any thread, once per
   * exchange. Returns its address, or zero when the budget is exhausted.
   */
  long httpPrepare(long exchange, long capacity);

  /**
   * Send the first {@code length} bytes of the prepared buffer as the whole response. {@code flags}
   * bit 0 asks the driver to close the connection afterwards. Driver thread only.
   */
  int httpSend(long driver, long exchange, long length, int flags);

  /**
   * Stop charging a body segment against its socket's receive window, leaving its bytes valid at
   * the same address. Driver thread only; idempotent.
   *
   * <p>The window bounds the transport's read-ahead. A consumer acks once the segment's bytes have
   * become its own memory — handed to a guest, or otherwise held under the consumer's own body cap
   * rather than the transport's — so they stop throttling the socket even though nothing has been
   * freed. A segment the consumer is merely holding must not be acked, or the window becomes a
   * no-op.
   */
  int httpSegmentAck(long driver, long segment);

  /**
   * Release a body segment delivered by an {@link #EVENT_BODY} event: its bytes are invalid
   * afterwards. Driver thread only, exactly once per event. Releasing also acks, so releasing
   * resumes receiving on the segment's socket once enough capacity has been freed.
   */
  int httpSegmentRelease(long driver, long segment);

  /** Flag for {@link #httpChunkSend}: this part ends the response. */
  int CHUNK_FINAL = 2;

  /** Most response bytes one streaming exchange may hold queued or in flight. */
  int RESPONSE_WINDOW_BYTES = 256 * 1024;

  /**
   * Allocate storage for one response part of a streaming exchange ({@link #RESPOND_STREAM}):
   * {@code capacity} payload bytes the caller fills at the address written to {@code addressOut},
   * framed in place by {@link #httpChunkSend}. Any thread, while the exchange is unfreed; several
   * parts may be prepared ahead. Returns the buffer handle, which the caller owns until {@code
   * chunkSend} takes it, or zero on failure.
   */
  long httpChunkPrepare(long exchange, long capacity, long addressOut);

  /**
   * Queue the first {@code length} payload bytes of a prepared part on a streaming exchange, framed
   * per the response; {@link #CHUNK_FINAL} in {@code flags} ends the response. Driver thread only.
   *
   * <p>Return value determines who owns {@code buffer} next: {@code 0} queues the part and the
   * driver takes ownership of the buffer; {@code -1} ({@code INVALID}) leaves the buffer with the
   * caller (bad handle, a length beyond the prepared capacity, or an exchange that is not
   * streaming); {@code -2} ({@code BUSY}) also leaves the buffer with the caller because the
   * exchange already holds {@link #RESPONSE_WINDOW_BYTES} queued or in flight — wait for {@link
   * #EVENT_PART_SENT} and retry, or the buffer leaks; any other negative value is a portable error
   * from a payload that diverges from a declared {@code content-length}, and the buffer is consumed
   * as the connection fails.
   */
  int httpChunkSend(long driver, long exchange, long buffer, long length, int flags);

  /** A direct view over {@code length} bytes of native memory at {@code address}; no copy. */
  ByteBuffer memory(long address, int length);

  /**
   * Read the eight bytes at {@code address} in native byte order. Unsafe by contract: the caller
   * guarantees the address is live, readable and eight-byte aligned. Nothing is bounds-checked, and
   * no object is allocated — this is the primitive for reading a fixed-size native layout on a hot
   * path, where {@link #memory} would allocate a view per read.
   */
  long getLong(long address);

  /**
   * Read the four bytes at {@code address} in native byte order. Same contract as {@link
   * #getLong(long)}, with four-byte alignment.
   */
  int getInt(long address);

  /** Read the byte at {@code address}. Same contract as {@link #getLong(long)}. */
  byte getByte(long address);

  /**
   * Copy live native bytes into caller-owned storage. The caller guarantees a readable source;
   * destination bounds are checked before copying. No native-memory view is retained.
   */
  default void copyBytes(long address, byte[] target, int offset, int length) {
    java.util.Objects.checkFromIndexSize(offset, length, target.length);
    memory(address, length).get(target, offset, length);
  }

  /** Copy checked managed bytes into caller-owned, live writable native storage. */
  default void writeBytes(long address, byte[] source, int offset, int length) {
    java.util.Objects.checkFromIndexSize(offset, length, source.length);
    memory(address, length).put(source, offset, length);
  }

  /** Raw address of a buffer's storage; stable for the handle's lifetime. */
  long bufferAddress(long buffer);

  /** Copy {@code length} bytes from native memory at {@code address}. */
  byte[] bytes(long address, int length);

  long tlsClient(long workload, long roots, long alpn);

  long tlsServer(long workload, long chain, long key, long alpn);

  int tlsContextRelease(long context);

  /** Once the workload closes, feeds and writes fail; progress and close-notify remain. */
  long tlsNew(long workload, long context, long owner, long name);

  int tlsFeed(long session, long input, long length);

  int tlsStep(long session, int action, long plaintext, long offset, long length, long output);

  int tlsProtocol(long session, long output);

  int tlsRelease(long session);

  // ---- SSLEngine (begin) ----------------------------------------------------------------------

  /** Engine context flag: server chain and key rather than client trust anchors. */
  int ENGINE_SERVER = 1;

  /** Engine context flag: concatenated DER certificates and a DER key rather than PEM. */
  int ENGINE_DER = 2;

  /** Client-only context flag: explicitly disable certificate-chain and hostname verification. */
  int ENGINE_INSECURE = 4;

  /** Context flag: ALPN has a versioned protocol/cipher policy prefix. */
  int ENGINE_POLICY = 8;

  /** Client context flag: key contains a PEM client certificate chain and private key. */
  int ENGINE_CLIENT_IDENTITY = 16;

  /** Client context flag: key contains a versioned identity list with issuer-name hints. */
  int ENGINE_CLIENT_IDENTITIES = 32;

  /** Engine result: terminal TLS failure, described by {@link #ENGINE_INFO_FAILURE}. */
  long ENGINE_FAILED = -3;

  int ENGINE_INFO_ALPN = 0;
  int ENGINE_INFO_PROTOCOL = 1;
  int ENGINE_INFO_SUITE = 2;
  int ENGINE_INFO_PEER_COUNT = 3;
  int ENGINE_INFO_PEER = 4;
  int ENGINE_INFO_FAILURE = 5;
  int ENGINE_INFO_SESSION_ID = 6;
  int ENGINE_INFO_LOCAL_COUNT = 7;
  int ENGINE_INFO_LOCAL = 8;

  int ENGINE_CONTROL_STATE = 0;
  int ENGINE_CONTROL_BEGIN = 1;
  int ENGINE_CONTROL_CLOSE_OUTBOUND = 2;
  int ENGINE_CONTROL_CLOSE_INBOUND = 3;

  /** Whether the loaded library exports the engine ABI. */
  default boolean supportsEngine() {
    return false;
  }

  /** Immutable Rustls context; inputs are read during the call only. Zero on invalid input. */
  default long engineContextNew(
      long workload,
      int flags,
      byte @Nullable [] certificates,
      byte @Nullable [] key,
      byte @Nullable [] alpn) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  default int engineContextRelease(long context) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  /** Clients pass a UTF-8 DNS name or IP literal; servers pass null. Zero on invalid input. */
  default long engineNew(long workload, long context, byte @Nullable [] name) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  /**
   * Encrypt {@code sourceLength} bytes at the source position into {@code destinationLength} bytes
   * at the destination position; positions do not move. Direct buffers pass their addresses. Heap
   * buffers must expose arrays and are accessed in place, blocking safepoints for the call, so
   * callers pass them only for bounded work. Zero lengths pass no memory. Returns the packed result
   * from {@code elide_transport_engine_wrap}.
   */
  default long engineWrap(
      long engine,
      @Nullable ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  /**
   * Process at most one record, with {@link #engineWrap}'s buffer rules. A writable source may be
   * decrypted in place; read-only sources are staged natively.
   */
  default long engineUnwrap(
      long engine,
      ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  default long engineControl(long engine, int operation) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  /** Copy up to {@code output.length} bytes of one session detail; returns its full length. */
  default int engineInfo(long engine, int kind, int index, byte @Nullable [] output) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  default int engineRelease(long engine) {
    throw new UnsupportedOperationException("Native TLS engines are unavailable");
  }

  // ---- SSLEngine (end) ------------------------------------------------------------------------
}
