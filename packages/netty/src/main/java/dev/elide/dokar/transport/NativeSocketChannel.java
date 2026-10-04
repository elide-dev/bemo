/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.dokar.transport;

import io.netty.channel.socket.ServerSocketChannel;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.SocketChannelConfig;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import org.jspecify.annotations.Nullable;

/** Completion-driven TCP channel backed by the portable Rust transport ABI. */
public final class NativeSocketChannel extends NativeStreamChannel implements SocketChannel {
  public NativeSocketChannel() {}

  NativeSocketChannel(NativeServerSocketChannel parent, TransportNative api, long accepted) {
    super(parent, api, accepted);
  }

  @Override
  public NativeSocketChannel tls(NativeTlsContext context, @Nullable String peerName) {
    super.tls(context, peerName);
    return this;
  }

  @Override
  public ServerSocketChannel parent() {
    return (ServerSocketChannel) super.parent();
  }

  @Override
  public SocketChannelConfig config() {
    return settings;
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
  long endpoint(SocketAddress address) {
    return io().endpoint((InetSocketAddress) address);
  }
}
