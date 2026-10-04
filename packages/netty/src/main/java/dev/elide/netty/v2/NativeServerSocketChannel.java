/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.netty.v2;

import io.netty.channel.*;
import io.netty.channel.socket.*;
import java.net.InetSocketAddress;
import java.net.SocketAddress;

/** TCP listener whose accepted sockets transfer directly between native event-loop owners. */
@SuppressWarnings("deprecation")
public final class NativeServerSocketChannel extends NativeChannel implements ServerSocketChannel {
  private long accept;

  public NativeServerSocketChannel() {
    super(null);
  }

  @Override
  public InetSocketAddress localAddress() {
    return (InetSocketAddress) super.localAddress();
  }

  @Override
  public InetSocketAddress remoteAddress() {
    return (InetSocketAddress) super.remoteAddress();
  }

  @Override
  public ServerSocketChannelConfig config() {
    return settings;
  }

  @Override
  protected void doBind(SocketAddress address) {
    long endpoint = io.endpoint((InetSocketAddress) address);
    try {
      socket =
          io.api.socketListen(
              workload,
              io.driver(),
              endpoint,
              settings.getBacklog(),
              settings.isReuseAddress() ? 1 : 0);
    } finally {
      io.api.bufferRelease(endpoint);
    }
    if (socket == 0) throw NativeTransportException.operation("bind", io.api.lastError());
    io.associate(socket, registration);
    settings.apply();
    refreshAddresses();
    active = true;
  }

  @Override
  void beginNativeRead() {
    if (!active || accept != 0 || !(readRequested || settings.isAutoRead())) return;
    accept = io.api.socketAccept(workload, io.driver(), socket);
    if (accept == 0) throw new NativeTransportException("Native accept admission failed");
  }

  @Override
  void completed(NativeIoHandler.Completion event) {
    if (event.kind != 2 || event.operation != accept) return;
    accept = 0;
    if (!open) return;
    if (event.result < 0) throw NativeTransportException.operation("accept", event.result);
    NativeSocketChannel child = new NativeSocketChannel(this, io.api, event.value);
    event.value = 0;
    readRequested = false;
    pipeline().fireChannelRead(child);
    pipeline().fireChannelReadComplete();
    beginNativeRead();
  }

  @Override
  protected void doWrite(ChannelOutboundBuffer outbound) {
    throw new UnsupportedOperationException("Listener write");
  }
}
