/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import io.netty.buffer.ByteBuf;
import io.netty.channel.*;
import io.netty.channel.socket.*;
import io.netty.util.concurrent.ScheduledFuture;
import java.net.SocketAddress;
import java.nio.ByteBuffer;
import java.nio.channels.AlreadyConnectedException;
import java.nio.channels.ClosedChannelException;
import java.nio.channels.ConnectionPendingException;
import java.util.concurrent.TimeUnit;
import org.jspecify.annotations.Nullable;

/** Shared byte-stream lifecycle for native TCP and Unix sockets. */
@SuppressWarnings("deprecation")
abstract class NativeStreamChannel extends NativeChannel implements DuplexChannel {
  public enum TlsClosed {
    INSTANCE
  }

  private @Nullable NativeTlsContext tlsContext;
  private @Nullable String tlsPeerName;
  private @Nullable NativeTlsSession tls;
  private TransportEvents.@Nullable TlsHandshake tlsFlight;
  private volatile @Nullable String applicationProtocol;
  private io.netty.util.concurrent.@Nullable Promise<Void> handshake;
  private final @Nullable TransportNative acceptedApi;
  private long accepted;
  private @Nullable ChannelPromise connectPromise;
  private @Nullable ScheduledFuture<?> connectTimeout;
  private @Nullable ScheduledFuture<?> handshakeTimeout;
  private long receive;
  private int receiveCapacity;
  private boolean dispatchingRead;
  private boolean completingConnect;
  private final NativeIoHandler.Completion immediateRead = new NativeIoHandler.Completion();
  private long send;
  private long stagedWrite;
  private int sentLength;
  private final long[] sendRegions = new long[64 * 3];
  private final @Nullable NativeByteBuf[] sendBuffers = new NativeByteBuf[64];
  private boolean borrowDirect;
  private int borrowedBytes;
  private final ChannelOutboundBuffer.MessageProcessor collectBorrowed =
      message -> {
        ByteBuf bytes = (ByteBuf) message;
        borrowDirect &=
            bytes.refCnt() == 1
                && bytes.isDirect()
                && (!(bytes instanceof NativeByteBuf nativeBytes) || !nativeBytes.isFrozen());
        borrowedBytes += Math.min(bytes.readableBytes(), 128 * 1024 - borrowedBytes);
        return borrowDirect && borrowedBytes < 128 * 1024;
      };
  private int sendRegionCount;
  private int sendRegionBytes;
  private final ChannelOutboundBuffer.MessageProcessor collectSendRegions = this::collectSendRegion;

  private boolean collectSendRegion(Object message) {
    if (!(message instanceof NativeByteBuf bytes)
        || !bytes.belongsTo(io().api)
        || bytes.refCnt() != 1) return false;
    if (!bytes.isReadable()) return true;
    int length = Math.min(bytes.readableBytes(), 128 * 1024 - sendRegionBytes);
    int index = sendRegionCount * 3;
    sendBuffers[sendRegionCount++] = bytes;
    sendRegions[index + 1] = bytes.readerIndex();
    sendRegions[index + 2] = length;
    sendRegionBytes += length;
    return length == bytes.readableBytes() && sendRegionCount < 64 && sendRegionBytes < 128 * 1024;
  }

  private boolean ioStarted;
  private boolean inputShutdown;
  private boolean outputShutdown;

  public NativeStreamChannel() {
    super(null);
    acceptedApi = null;
  }

  NativeStreamChannel(NativeServerSocketChannel parent, TransportNative api, long accepted) {
    super(parent);
    this.acceptedApi = api;
    this.accepted = accepted;
    this.workload = parent.workload;
  }

  public NativeStreamChannel tls(NativeTlsContext context, @Nullable String peerName) {
    if (tlsContext != null || (active && (parent() == null || ioStarted)))
      throw new IllegalStateException("Configure TLS before socket I/O");
    tlsContext = java.util.Objects.requireNonNull(context);
    tlsPeerName = peerName;
    if (active) startTls();
    return this;
  }

  public synchronized io.netty.util.concurrent.Future<Void> handshakeFuture() {
    initializeHandshake();
    return java.util.Objects.requireNonNull(handshake);
  }

  private synchronized void initializeHandshake() {
    if (handshake == null) {
      handshake = eventLoop().newPromise();
      if (!open) handshake.tryFailure(new ClosedChannelException());
    }
  }

  public @Nullable String applicationProtocol() {
    return applicationProtocol;
  }

  private void startTls() {
    if (tlsContext != null && tls == null) {
      initializeHandshake();
      tlsFlight = TransportEvents.handshake(this);
      tls = new NativeTlsSession(this, tlsContext, tlsPeerName);
      handshakeTimeout =
          eventLoop()
              .schedule(
                  () -> failTls(new NativeTransportException("TLS handshake timed out")),
                  10,
                  TimeUnit.SECONDS);
      tls.pump();
    }
  }

  @Override
  boolean deferClose(ChannelPromise promise) {
    if (tls != null && tls.ready() && open && !outputShutdown) {
      tls.requestClose(promise);
      return true;
    }
    return false;
  }

  void finishTlsOutputShutdown(ChannelPromise promise) {
    ((NativeUnsafe) unsafe()).shutdownOutput(promise);
  }

  void finishTlsClose(ChannelPromise promise) {
    ((NativeUnsafe) unsafe()).forceClose(promise);
  }

  void failTls(Throwable error) {
    finishTlsFlight("error", null);
    if (handshake != null) handshake.tryFailure(error);
    pipeline().fireExceptionCaught(error);
    ((NativeUnsafe) unsafe()).forceClose(voidPromise());
  }

  void tlsReady(@Nullable String protocol) {
    if (handshakeTimeout != null) {
      handshakeTimeout.cancel(false);
      handshakeTimeout = null;
    }
    applicationProtocol = protocol;
    finishTlsFlight("success", protocol);
    java.util.Objects.requireNonNull(handshake, "TLS handshake promise must exist")
        .trySuccess(null);
  }

  private void finishTlsFlight(String outcome, @Nullable String protocol) {
    TransportEvents.handshakeDone(tlsFlight, outcome, protocol);
    tlsFlight = null;
  }

  void sendTls(long buffer, int offset, int length) {
    send = io().api.socketSend(workload, io().driver(), socket, buffer, offset, length);
    if (send == 0) throw new NativeTransportException("TLS send admission failed");
  }

  void deliverTls(ByteBuf bytes) {
    readRequested = false;
    pipeline().fireChannelRead(bytes);
    pipeline().fireChannelReadComplete();
  }

  void tlsPeerClosed() {
    inputShutdown = true;
    pipeline().fireUserEventTriggered(TlsClosed.INSTANCE);
    if (settings.isAllowHalfClosure())
      pipeline().fireUserEventTriggered(ChannelInputShutdownEvent.INSTANCE);
    else unsafe().close(voidPromise());
  }

  @Override
  public ChannelConfig config() {
    return settings;
  }

  @Override
  // Accepted handles must retain the identical binding instance.
  @SuppressWarnings("ReferenceEquality")
  void registeredNative() {
    if (accepted == 0) return;
    if (io().api != acceptedApi)
      throw new NativeTransportException("Accepted socket belongs to another native library");
    long handle = accepted;
    accepted = 0;
    if (io().api.socketAdopt(workload, io().driver(), handle) != 0) {
      io().api.socketDiscard(handle);
      throw new NativeTransportException("Native socket adoption failed");
    }
    socket = handle;
    io().associate(socket, registration());
    settings.apply();
    refreshAddresses();
    active = true;
    startTls();
  }

  @Override
  protected void doBind(SocketAddress address) {
    throw new UnsupportedOperationException("Client local bind");
  }

  abstract long endpoint(SocketAddress remote);

  @Override
  void connectNative(SocketAddress remote, SocketAddress local, ChannelPromise promise)
      throws Exception {
    if (active) throw new AlreadyConnectedException();
    if (connectPromise != null) throw new ConnectionPendingException();
    if (local != null) throw new UnsupportedOperationException("Client local bind");
    long endpoint = endpoint(remote);
    try {
      socket = io().api.socketConnect(workload, io().driver(), endpoint);
    } finally {
      io().api.bufferRelease(endpoint);
    }
    if (socket == 0) throw NativeTransportException.operation("connect", io().api.lastError());
    io().associate(socket, registration());
    try {
      settings.apply();
    } catch (Throwable error) {
      io().close(socket);
      socket = 0;
      throw error;
    }
    connectPromise = promise;
    int timeout = settings.getConnectTimeoutMillis();
    if (timeout > 0)
      connectTimeout =
          eventLoop()
              .schedule(
                  () -> {
                    if (connectPromise != null) {
                      connectPromise.tryFailure(
                          new ConnectTimeoutException("Native connect timed out"));
                      unsafe().close(voidPromise());
                    }
                  },
                  timeout,
                  TimeUnit.MILLISECONDS);
  }

  @Override
  protected void doBeginRead() {
    readRequested = true;
    if (tls != null) tls.pump();
    beginNativeRead();
  }

  @Override
  void beginNativeRead() {
    if (dispatchingRead || completingConnect || (tls != null && tls.isPumping())) return;
    dispatchingRead = true;
    try {
      for (int messages = 0; messages < 16; messages++) {
        if (!active
            || (tlsContext != null && tls == null)
            || inputShutdown
            || receive != 0
            || !(readRequested || settings.isAutoRead() || (tls != null && !tls.ready()))) return;
        if (tls != null && tls.ready() && tls.hasPendingRead()) return;
        int capacity = Math.max(1, Math.min(1024 * 1024, unsafe().recvBufAllocHandle().guess()));
        // Private storage is uninitialized until the kernel fills it; custom allocators keep
        // control.
        if (settings.useAdaptiveReadFloor() && capacity >= 16 * 1024 && capacity < 64 * 1024)
          capacity = 64 * 1024;
        ioStarted = true;
        if (!io().api.supportsReceiveResults()) {
          receiveCapacity = capacity;
          receive =
              io().api
                  .socketReceiveNew(
                      workload, io().driver(), socket, io().allocator().owner, capacity);
          if (receive == 0) {
            receiveCapacity = 0;
            throw new NativeTransportException("Native receive allocation or admission failed");
          }
          return;
        }
        TransportNative.ReceiveResult result =
            io().api
                .socketReceiveNewResult(
                    workload, io().driver(), socket, io().allocator().owner, capacity);
        if (result.operation() != 0) {
          receive = result.operation();
          receiveCapacity = capacity;
          return;
        }
        immediateRead.value = result.buffer();
        immediateRead.result = result.result();
        try {
          deliverRead(immediateRead, capacity, result.bytes());
        } finally {
          if (immediateRead.value != 0) {
            io().api.bufferRelease(immediateRead.value);
            immediateRead.value = 0;
          }
        }
      }
      // Ready reads cannot monopolize the loop or recursively enter pipeline callbacks.
      eventLoop().execute(this::resumeNativeReads);
    } finally {
      dispatchingRead = false;
    }
  }

  private void resumeNativeReads() {
    try {
      beginNativeRead();
    } catch (Throwable error) {
      pipeline().fireExceptionCaught(error);
      ((NativeUnsafe) unsafe()).forceClose(voidPromise());
    }
  }

  private void deliverRead(
      NativeIoHandler.Completion event, int attempted, @Nullable ByteBuffer view) {
    if (inputShutdown) return;
    if (event.result < 0) throw NativeTransportException.operation("receive", event.result);
    RecvByteBufAllocator.Handle allocation = unsafe().recvBufAllocHandle();
    allocation.reset(settings);
    allocation.attemptedBytesRead(attempted);
    allocation.lastBytesRead(Math.toIntExact(event.result));
    if (event.result > 0) allocation.incMessagesRead(1);
    allocation.readComplete();
    if (tls != null) {
      tls.feed(event);
      return;
    }
    readRequested = false;
    if (event.result == 0) {
      inputShutdown = true;
      if (settings.isAllowHalfClosure())
        pipeline().fireUserEventTriggered(ChannelInputShutdownEvent.INSTANCE);
      else unsafe().close(voidPromise());
      return;
    }
    long handle = event.value;
    event.value = 0;
    ByteBuf bytes =
        view == null
            ? io().allocator().received(handle, Math.toIntExact(event.result))
            : io().allocator().received(handle, Math.toIntExact(event.result), view);
    pipeline().fireChannelRead(bytes);
    pipeline().fireChannelReadComplete();
  }

  @Override
  void completed(NativeIoHandler.Completion event) throws Exception {
    if (!open) return;
    switch (event.kind) {
      case 1 -> {
        ChannelPromise promise = connectPromise;
        if (promise == null) return;
        connectPromise = null;
        if (connectTimeout != null) {
          connectTimeout.cancel(false);
          connectTimeout = null;
        }
        if (event.result < 0) {
          promise.tryFailure(NativeTransportException.operation("connect", event.result));
          unsafe().close(voidPromise());
          return;
        }
        refreshAddresses();
        active = true;
        invalidateLocalAddress();
        invalidateRemoteAddress();
        completingConnect = true;
        try {
          promise.trySuccess();
          if (!open) return;
          startTls();
          if (open) pipeline().fireChannelActive();
        } finally {
          completingConnect = false;
        }
        beginNativeRead();
      }
      case 3 -> {
        if (event.operation != receive) return;
        int attempted = receiveCapacity;
        receive = 0;
        receiveCapacity = 0;
        dispatchingRead = true;
        try {
          deliverRead(event, attempted, null);
        } finally {
          dispatchingRead = false;
        }
        beginNativeRead();
      }
      case 4 -> {
        if (event.operation != send) return;
        send = 0;
        if (tls != null) {
          tls.sent(event.result);
          return;
        }
        if (stagedWrite != 0) {
          io().api.bufferRelease(stagedWrite);
          stagedWrite = 0;
        }
        if (event.result <= 0 || event.result > sentLength)
          throw NativeTransportException.operation("send", event.result);
        ChannelOutboundBuffer outbound = unsafe().outboundBuffer();
        if (outbound != null) {
          advanceWritten(outbound, event.result);
          ((NativeUnsafe) unsafe()).resumeWrites();
        }
      }
      default -> throw new NativeTransportException("Unexpected native completion");
    }
  }

  private static void advanceWritten(ChannelOutboundBuffer outbound, long written) {
    // Include the last partially written entry: maxBytes=written can omit its cached view.
    // Netty caches these views, so advance them before moving the ByteBuf indices.
    ByteBuffer[] buffers = outbound.nioBuffers(64, Long.MAX_VALUE);
    long remaining = written;
    for (int i = 0; i < outbound.nioBufferCount() && remaining > 0; i++) {
      ByteBuffer buffer = buffers[i];
      int length = (int) Math.min(buffer.remaining(), remaining);
      buffer.position(buffer.position() + length);
      remaining -= length;
    }
    outbound.removeBytes(written);
  }

  @Override
  protected Object filterOutboundMessage(Object message) {
    if (message instanceof ByteBuf) return message;
    throw new UnsupportedOperationException("Native TCP writes require ByteBuf");
  }

  @Override
  protected void doWrite(ChannelOutboundBuffer outbound) {
    // Connect listeners may flush before TLS starts; leave those writes queued for TLS.
    if (send != 0 || (tlsContext != null && tls == null)) return;
    ioStarted = true;
    if (tls != null) {
      tls.pump();
      return;
    }
    int spins = settings.getWriteSpinCount();
    for (; ; ) {
      ByteBuf bytes = (ByteBuf) outbound.current();
      if (bytes == null) return;
      if (!bytes.isReadable()) {
        outbound.remove();
        continue;
      }
      ByteBuffer[] buffers = outbound.nioBuffers(64, 128 * 1024);
      int count = outbound.nioBufferCount();
      long available = 0;
      for (int i = 0; i < count; i++) available += buffers[i].remaining();
      sentLength = (int) Math.min(available, 128 * 1024L);
      // An exclusively owned direct buffer can be borrowed through a bounded syscall.
      // Frozen buffers use their immutable native handle; never revive an old mutable view.
      if (count == 1
          && bytes.refCnt() == 1
          && bytes.isDirect()
          && bytes.nioBufferCount() == 1
          && (!(bytes instanceof NativeByteBuf nativeBytes) || !nativeBytes.isFrozen())) {
        long written =
            io().trySendInlineDirect(
                    workload,
                    socket,
                    bytes.internalNioBuffer(bytes.readerIndex(), bytes.readableBytes()));
        if (written < 0) throw NativeTransportException.operation("send", written);
        if (written > Math.min(bytes.readableBytes(), 128 * 1024))
          throw new NativeTransportException("Invalid inline send length");
        if (written > 0) {
          advanceWritten(outbound, written);
          if (--spins == 0) {
            eventLoop().execute(() -> ((NativeUnsafe) unsafe()).resumeWrites());
            return;
          }
          continue;
        }
      }
      if (io().api.supportsGatheredWrites()) {
        sendRegionCount = sendRegionBytes = 0;
        try {
          outbound.forEachFlushedMessage(collectSendRegions);
          if (sendRegionCount > 1) {
            for (int i = 0; i < sendRegionCount; i++)
              sendRegions[i * 3] = java.util.Objects.requireNonNull(sendBuffers[i]).freeze();
            sentLength = sendRegionBytes;
            send =
                io().api
                    .socketSendGathered(
                        workload, io().driver(), socket, sendRegions, sendRegionCount);
            if (send == 0)
              throw new NativeTransportException("Native gathered send admission failed");
            return;
          }
        } catch (RuntimeException error) {
          throw error;
        } catch (Exception error) {
          throw new IllegalStateException("Cannot gather native send regions", error);
        } finally {
          java.util.Arrays.fill(sendBuffers, 0, sendRegionCount, null);
        }
      }
      // A partial gathered write may leave one frozen region before non-native messages.
      // Submit that immutable handle without reading its abandoned mutable Java view.
      if (bytes instanceof NativeByteBuf nativeBytes
          && nativeBytes.isFrozen()
          && nativeBytes.belongsTo(io().api)
          && bytes.refCnt() == 1) {
        sentLength = Math.min(bytes.readableBytes(), 128 * 1024);
        send =
            io().api
                .socketSend(
                    workload,
                    io().driver(),
                    socket,
                    nativeBytes.freeze(),
                    bytes.readerIndex(),
                    sentLength);
        if (send == 0) throw new NativeTransportException("Native send admission failed");
        return;
      }
      // Gather flushed records into one bounded send instead of round-tripping through
      // native completion once per TLS record. Only this event loop can change the queue.
      long handle;
      int offset;
      if (count == 1
          && bytes instanceof NativeByteBuf nativeBytes
          && nativeBytes.belongsTo(io().api)
          && bytes.refCnt() == 1) {
        handle = nativeBytes.freeze();
        offset = bytes.readerIndex();
      } else {
        borrowDirect = io().api.supportsInlineVectoredWrites();
        borrowedBytes = 0;
        if (borrowDirect) {
          try {
            outbound.forEachFlushedMessage(collectBorrowed);
          } catch (Exception error) {
            throw new IllegalStateException("Cannot inspect borrowed writes", error);
          }
        }
        long written =
            io().trySendInline(workload, socket, buffers, count, sentLength, borrowDirect);
        if (written < 0) throw NativeTransportException.operation("send", written);
        if (written > sentLength) throw new NativeTransportException("Invalid inline send length");
        if (written > 0) {
          if (!borrowDirect) TransportEvents.copy(this, "tcp-write", (int) written);
          advanceWritten(outbound, written);
          if (--spins == 0) {
            eventLoop().execute(() -> ((NativeUnsafe) unsafe()).resumeWrites());
            return;
          }
          continue;
        }
        stagedWrite = io().api.bufferNew(io().allocator().owner, sentLength);
        if (stagedWrite == 0) throw new OutOfMemoryError("Native send budget exhausted");
        try {
          ByteBuffer target = io().api.bufferView(stagedWrite).limit(sentLength);
          for (int i = 0; i < count && target.hasRemaining(); i++) {
            ByteBuffer source = buffers[i];
            int length = Math.min(source.remaining(), target.remaining());
            target.put(target.position(), source, source.position(), length);
            target.position(target.position() + length);
          }
          if (target.hasRemaining())
            throw new IllegalStateException("Incomplete native send batch");
          if (io().api.bufferFreeze(stagedWrite, sentLength) != 0)
            throw new NativeTransportException("Native send freeze failed");
        } catch (Throwable error) {
          io().api.bufferRelease(stagedWrite);
          stagedWrite = 0;
          throw error;
        }
        TransportEvents.copy(this, "tcp-write", sentLength);
        handle = stagedWrite;
        offset = 0;
      }
      send = io().api.socketSend(workload, io().driver(), socket, handle, offset, sentLength);
      if (send == 0) {
        if (stagedWrite != 0) {
          io().api.bufferRelease(stagedWrite);
          stagedWrite = 0;
        }
        throw new NativeTransportException("Native send admission failed");
      }
      return;
    }
  }

  @Override
  protected void doClose() throws Exception {
    finishTlsFlight("closed", null);
    if (handshakeTimeout != null) {
      handshakeTimeout.cancel(false);
      handshakeTimeout = null;
    }
    if (tls != null) {
      tls.close();
      tls = null;
    }
    if (accepted != 0) {
      java.util.Objects.requireNonNull(acceptedApi, "accepted sockets retain their native binding")
          .socketDiscard(accepted);
      accepted = 0;
    }
    if (connectTimeout != null) {
      connectTimeout.cancel(false);
      connectTimeout = null;
    }
    if (connectPromise != null) {
      connectPromise.tryFailure(new ClosedChannelException());
      connectPromise = null;
    }
    if (stagedWrite != 0) {
      io().api.bufferRelease(stagedWrite);
      stagedWrite = 0;
    }
    inputShutdown = outputShutdown = true;
    try {
      super.doClose();
    } finally {
      synchronized (this) {
        if (handshake != null) handshake.tryFailure(new ClosedChannelException());
      }
    }
  }

  @Override
  protected void doShutdownOutput() {
    if (io().api.socketShutdown(io().driver(), socket, 1) != 0)
      throw new NativeTransportException("Native output shutdown failed");
    outputShutdown = true;
  }

  @Override
  public boolean isInputShutdown() {
    return inputShutdown || !open;
  }

  @Override
  public boolean isOutputShutdown() {
    return outputShutdown || !open;
  }

  @Override
  public boolean isShutdown() {
    return isInputShutdown() && isOutputShutdown();
  }

  @Override
  public ChannelFuture shutdownInput() {
    return shutdownInput(newPromise());
  }

  @Override
  public ChannelFuture shutdownOutput() {
    return shutdownOutput(newPromise());
  }

  @Override
  public ChannelFuture shutdown() {
    return shutdown(newPromise());
  }

  @Override
  public ChannelFuture shutdownInput(ChannelPromise promise) {
    return shutdown(0, promise);
  }

  @Override
  public ChannelFuture shutdownOutput(ChannelPromise promise) {
    return shutdown(1, promise);
  }

  @Override
  public ChannelFuture shutdown(ChannelPromise promise) {
    return shutdown(2, promise);
  }

  private ChannelFuture shutdown(int direction, ChannelPromise promise) {
    Runnable operation =
        () -> {
          if (!promise.setUncancellable()) return;
          if (!active || (direction != 0 && send != 0)) {
            promise.tryFailure(new NativeTransportException("Socket inactive or write pending"));
            return;
          }
          if (direction != 1
              && !inputShutdown
              && io().api.socketShutdown(io().driver(), socket, 0) != 0) {
            promise.tryFailure(new NativeTransportException("Native shutdown failed"));
            return;
          }
          if (direction != 1) inputShutdown = true;
          if (direction != 0 && !outputShutdown) {
            if (tls != null) tls.requestOutputShutdown(promise);
            else ((NativeUnsafe) unsafe()).shutdownOutput(promise);
          } else promise.trySuccess();
        };
    if (eventLoop().inEventLoop()) operation.run();
    else eventLoop().execute(operation);
    return promise;
  }
}
