/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.dokar.transport;

import io.netty.buffer.ByteBuf;
import io.netty.channel.*;
import io.netty.channel.socket.*;
import io.netty.util.concurrent.ScheduledFuture;
import java.net.SocketAddress;
import java.nio.channels.AlreadyConnectedException;
import java.nio.channels.ClosedChannelException;
import java.nio.channels.ConnectionPendingException;
import java.util.concurrent.TimeUnit;

/** Shared byte-stream lifecycle for native TCP and Unix sockets. */
@SuppressWarnings("deprecation")
abstract class NativeStreamChannel extends NativeChannel implements DuplexChannel {
  public enum TlsClosed {
    INSTANCE
  }

  private NativeTlsContext tlsContext;
  private String tlsPeerName;
  private NativeTlsSession tls;
  private TransportEvents.TlsHandshake tlsFlight;
  private volatile String applicationProtocol;
  private io.netty.util.concurrent.Promise<Void> handshake;
  private final TransportNative acceptedApi;
  private long accepted;
  private ChannelPromise connectPromise;
  private ScheduledFuture<?> connectTimeout;
  private ScheduledFuture<?> handshakeTimeout;
  private long receive;
  private int receiveCapacity;
  private long send;
  private long stagedWrite;
  private int sentLength;
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

  public NativeStreamChannel tls(NativeTlsContext context, String peerName) {
    if (tlsContext != null || active && (parent() == null || ioStarted))
      throw new IllegalStateException("Configure TLS before socket I/O");
    tlsContext = java.util.Objects.requireNonNull(context);
    tlsPeerName = peerName;
    if (active) startTls();
    return this;
  }

  public synchronized io.netty.util.concurrent.Future<Void> handshakeFuture() {
    if (handshake == null) {
      handshake = eventLoop().newPromise();
      if (!open) handshake.tryFailure(new ClosedChannelException());
    }
    return handshake;
  }

  public String applicationProtocol() {
    return applicationProtocol;
  }

  private void startTls() {
    if (tlsContext != null && tls == null) {
      handshakeFuture();
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

  void tlsReady(String protocol) {
    if (handshakeTimeout != null) {
      handshakeTimeout.cancel(false);
      handshakeTimeout = null;
    }
    applicationProtocol = protocol;
    finishTlsFlight("success", protocol);
    handshake.trySuccess(null);
  }

  private void finishTlsFlight(String outcome, String protocol) {
    TransportEvents.handshakeDone(tlsFlight, outcome, protocol);
    tlsFlight = null;
  }

  void sendTls(long buffer, int offset, int length) {
    send = io.api.socketSend(workload, io.driver(), socket, buffer, offset, length);
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
  void registeredNative() {
    if (accepted == 0) return;
    if (io.api != acceptedApi)
      throw new NativeTransportException("Accepted socket belongs to another native library");
    long handle = accepted;
    accepted = 0;
    if (io.api.socketAdopt(workload, io.driver(), handle) != 0) {
      io.api.socketDiscard(handle);
      throw new NativeTransportException("Native socket adoption failed");
    }
    socket = handle;
    io.associate(socket, registration);
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
      socket = io.api.socketConnect(workload, io.driver(), endpoint);
    } finally {
      io.api.bufferRelease(endpoint);
    }
    if (socket == 0) throw NativeTransportException.operation("connect", io.api.lastError());
    io.associate(socket, registration);
    try {
      settings.apply();
    } catch (Throwable error) {
      io.close(socket);
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
    if (!active
        || tlsContext != null && tls == null
        || inputShutdown
        || receive != 0
        || !(readRequested || settings.isAutoRead() || tls != null && !tls.ready())) return;
    if (tls != null && tls.ready() && tls.hasPendingRead()) return;
    int capacity = Math.max(1, Math.min(1024 * 1024, unsafe().recvBufAllocHandle().guess()));
    ioStarted = true;
    receiveCapacity = capacity;
    receive = io.api.socketReceiveNew(workload, io.driver(), socket, io.allocator.owner, capacity);
    if (receive == 0) {
      receiveCapacity = 0;
      throw new NativeTransportException("Native receive allocation or admission failed");
    }
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
        promise.trySuccess();
        if (!open) return;
        startTls();
        if (open) pipeline().fireChannelActive();
      }
      case 3 -> {
        if (event.operation != receive) return;
        int attempted = receiveCapacity;
        receive = 0;
        receiveCapacity = 0;
        if (inputShutdown) return;
        if (event.result < 0) throw NativeTransportException.operation("receive", event.result);
        RecvByteBufAllocator.Handle allocation = unsafe().recvBufAllocHandle();
        allocation.reset(settings);
        allocation.attemptedBytesRead(attempted);
        allocation.lastBytesRead(Math.toIntExact(event.result));
        if (event.result > 0) allocation.incMessagesRead(1);
        // TLS pumping and channelRead callbacks can submit the next receive immediately.
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
        ByteBuf bytes = io.allocator.received(handle, Math.toIntExact(event.result));
        pipeline().fireChannelRead(bytes);
        pipeline().fireChannelReadComplete();
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
          io.api.bufferRelease(stagedWrite);
          stagedWrite = 0;
        }
        if (event.result <= 0 || event.result > sentLength)
          throw NativeTransportException.operation("send", event.result);
        ChannelOutboundBuffer outbound = unsafe().outboundBuffer();
        if (outbound != null) {
          outbound.removeBytes(event.result);
          ((NativeUnsafe) unsafe()).resumeWrites();
        }
      }
      default -> throw new NativeTransportException("Unexpected native completion");
    }
  }

  @Override
  protected Object filterOutboundMessage(Object message) {
    if (message instanceof ByteBuf) return message;
    throw new UnsupportedOperationException("Native TCP writes require ByteBuf");
  }

  @Override
  protected void doWrite(ChannelOutboundBuffer outbound) {
    // Connect listeners may flush before TLS starts; leave those writes queued for TLS.
    if (send != 0 || tlsContext != null && tls == null) return;
    ioStarted = true;
    if (tls != null) {
      tls.pump();
      return;
    }
    for (; ; ) {
      ByteBuf bytes = (ByteBuf) outbound.current();
      if (bytes == null) return;
      if (!bytes.isReadable()) {
        outbound.remove();
        continue;
      }
      sentLength = Math.min(bytes.readableBytes(), 64 * 1024);
      long handle;
      int offset;
      if (bytes instanceof NativeByteBuf nativeBytes
          && nativeBytes.belongsTo(io.api)
          && bytes.refCnt() == 1) {
        handle = nativeBytes.freeze();
        offset = bytes.readerIndex();
      } else {
        stagedWrite = io.api.bufferNew(io.allocator.owner, sentLength);
        if (stagedWrite == 0) throw new OutOfMemoryError("Native send budget exhausted");
        try {
          bytes.getBytes(bytes.readerIndex(), io.api.bufferView(stagedWrite).limit(sentLength));
          if (io.api.bufferFreeze(stagedWrite, sentLength) != 0)
            throw new NativeTransportException("Native send freeze failed");
        } catch (Throwable error) {
          io.api.bufferRelease(stagedWrite);
          stagedWrite = 0;
          throw error;
        }
        TransportEvents.copy(this, "tcp-write", sentLength);
        handle = stagedWrite;
        offset = 0;
      }
      send = io.api.socketSend(workload, io.driver(), socket, handle, offset, sentLength);
      if (send == 0) {
        if (stagedWrite != 0) {
          io.api.bufferRelease(stagedWrite);
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
      acceptedApi.socketDiscard(accepted);
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
      io.api.bufferRelease(stagedWrite);
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
    if (io.api.socketShutdown(io.driver(), socket, 1) != 0)
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
              && io.api.socketShutdown(io.driver(), socket, 0) != 0) {
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
