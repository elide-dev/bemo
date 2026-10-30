/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

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
    long endpoint = io().endpoint((InetSocketAddress) address);
    try {
      socket =
          io().api
              .socketListen(
                  workload,
                  io().driver(),
                  endpoint,
                  settings.getBacklog(),
                  settings.isReuseAddress() ? 1 : 0);
    } finally {
      io().api.bufferRelease(endpoint);
    }
    if (socket == 0) throw NativeTransportException.operation("bind", io().api.lastError());
    io().associate(socket, registration());
    settings.apply();
    refreshAddresses();
    active = true;
  }

  @Override
  void beginNativeRead() {
    if (!active || accept != 0 || !(readRequested || settings.isAutoRead())) return;
    long pending = io().api.socketAccept(workload, io().driver(), socket);
    // Admission rejection (the driver's transient operation limit is reached) yields 0, not a
    // negative completion. Returning instead of throwing keeps the listener open: a throw
    // propagates to NativeUnsafe.handle (or AbstractUnsafe.beginRead) and force-closes the
    // server channel. Arming retries on the next doBeginRead / auto-read once capacity frees.
    if (pending == 0) return;
    accept = pending;
  }

  @Override
  void completed(NativeIoHandler.Completion event) {
    if (event.kind != 2 || event.operation != accept) return;
    accept = 0;
    if (!open) return;
    if (event.result < 0) {
      // A failed accept is a per-connection rejection (EMFILE/ENFILE, ECONNABORTED, transport
      // quota exhaustion), not a listener fault: discard the failed connection and keep
      // accepting. Mirrors AbstractNioMessageChannel, which catches per-accept IOException
      // (including "too many open files") and leaves the server channel open. The native side
      // reports value == 0 on failure, so there is no adopted handle to release here; the
      // dispatcher discards any non-zero value left on the completion.
      pipeline().fireExceptionCaught(NativeTransportException.operation("accept", event.result));
      readRequested = false;
      beginNativeRead();
      return;
    }
    NativeSocketChannel child = new NativeSocketChannel(this, io().api, event.value);
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
