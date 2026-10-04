/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.netty.v2;

import io.netty.channel.*;
import java.net.SocketAddress;

@SuppressWarnings("deprecation")
abstract class NativeChannel extends AbstractChannel {
  final NativeChannelConfig settings = new NativeChannelConfig(this);
  NativeIoHandler io;
  IoRegistration registration;
  volatile long socket;

  /** Owning workload, fixed at registration; accepted children inherit their listener's. */
  long workload;

  SocketAddress local;
  SocketAddress remote;
  volatile boolean open = true;
  volatile boolean active;
  boolean readRequested;

  NativeChannel(Channel parent) {
    super(parent);
  }

  @Override
  public boolean isOpen() {
    return open;
  }

  @Override
  public boolean isActive() {
    return active;
  }

  @Override
  public ChannelMetadata metadata() {
    return new ChannelMetadata(false);
  }

  @Override
  protected AbstractUnsafe newUnsafe() {
    return new NativeUnsafe();
  }

  @Override
  protected boolean isCompatible(EventLoop loop) {
    return loop instanceof IoEventLoop && ((IoEventLoop) loop).isCompatible(NativeUnsafe.class);
  }

  @Override
  protected SocketAddress localAddress0() {
    return local;
  }

  @Override
  protected SocketAddress remoteAddress0() {
    return remote;
  }

  @Override
  protected void doRegister(ChannelPromise promise) {
    ((IoEventLoop) eventLoop())
        .register((NativeUnsafe) unsafe())
        .addListener(
            future -> {
              if (!future.isSuccess()) {
                promise.tryFailure(future.cause());
                return;
              }
              registration = (IoRegistration) future.getNow();
              io = ((NativeIoHandler.Registration) registration.attachment()).owner;
              if (workload == 0) workload = io.workload();
              try {
                settings.installAllocator(io.allocator);
                registeredNative();
                promise.trySuccess();
              } catch (Throwable error) {
                registration.cancel();
                promise.tryFailure(error);
              }
            });
  }

  void refreshAddresses() {
    local = io.address(socket, false);
    remote = io.address(socket, true);
    invalidateLocalAddress();
    invalidateRemoteAddress();
  }

  void registeredNative() throws Exception {}

  @Override
  protected void doDeregister() {
    // Live driver migration requires an explicit drain; deregistration closes this channel.
    if (open) ((NativeUnsafe) unsafe()).forceClose(voidPromise());
    if (registration != null) {
      registration.cancel();
      registration = null;
    }
  }

  @Override
  protected void doDisconnect() throws Exception {
    doClose();
  }

  @Override
  protected void doClose() throws Exception {
    open = false;
    active = false;
    if (socket != 0) {
      io.close(socket);
      socket = 0;
    }
  }

  @Override
  protected void doBeginRead() throws Exception {
    readRequested = true;
    beginNativeRead();
  }

  abstract void beginNativeRead() throws Exception;

  abstract void completed(NativeIoHandler.Completion event) throws Exception;

  boolean deferClose(ChannelPromise promise) {
    return false;
  }

  void connectNative(SocketAddress remote, SocketAddress local, ChannelPromise promise)
      throws Exception {
    throw new UnsupportedOperationException("Connect is unsupported on this channel");
  }

  final class NativeUnsafe extends AbstractUnsafe implements NativeIoHandler.Handle {
    @Override
    public void connect(SocketAddress remote, SocketAddress local, ChannelPromise promise) {
      if (!promise.setUncancellable() || !ensureOpen(promise)) return;
      try {
        connectNative(remote, local, promise);
      } catch (Throwable error) {
        safeSetFailure(promise, error);
        closeIfClosed();
      }
    }

    @Override
    public void handle(IoRegistration registration, IoEvent event) {
      try {
        completed((NativeIoHandler.Completion) event);
      } catch (Throwable error) {
        pipeline().fireExceptionCaught(error);
        forceClose(voidPromise());
      }
    }

    @Override
    public void close(ChannelPromise promise) {
      if (!deferClose(promise)) super.close(promise);
    }

    void forceClose(ChannelPromise promise) {
      super.close(promise);
    }

    void resumeWrites() {
      flush0();
    }

    @Override
    public void close() {
      forceClose(voidPromise());
    }
  }
}
