/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.*;
import io.netty.channel.socket.*;
import java.util.Map;

@SuppressWarnings("deprecation")
final class NativeChannelConfig extends DefaultChannelConfig
    implements SocketChannelConfig, ServerSocketChannelConfig {
  private final NativeChannel nativeChannel;
  private volatile boolean tcpNoDelay = true;
  private volatile boolean keepAlive;
  private volatile boolean reuseAddress = true;
  private volatile boolean halfClosure;
  private volatile int receiveBuffer;
  private volatile int sendBuffer;
  private volatile int backlog = 128;
  private volatile boolean allocatorSet;
  private volatile boolean receiveAllocatorSet;

  boolean useAdaptiveReadFloor() {
    return !receiveAllocatorSet;
  }

  void installAllocator(ByteBufAllocator allocator) {
    if (!allocatorSet) super.setAllocator(allocator);
  }

  NativeChannelConfig(NativeChannel channel) {
    super(channel);
    this.nativeChannel = channel;
  }

  void apply() {
    if (channel instanceof NativeSocketChannel) {
      option(1, tcpNoDelay ? 1 : 0);
      option(2, keepAlive ? 1 : 0);
    }
    if (receiveBuffer > 0) option(3, receiveBuffer);
    if (sendBuffer > 0) option(4, sendBuffer);
    option(5, reuseAddress ? 1 : 0);
  }

  private void option(int option, int value) {
    if (nativeChannel.socket != 0 && !nativeChannel.eventLoop().inEventLoop()) {
      nativeChannel.eventLoop().submit(() -> option(option, value)).syncUninterruptibly();
      return;
    }
    if (nativeChannel.socket != 0
        && nativeChannel
                .io()
                .api
                .socketOption(nativeChannel.io().driver(), nativeChannel.socket, option, value)
            != 0) throw new NativeTransportException("Native socket option failed");
  }

  @Override
  public boolean isTcpNoDelay() {
    return tcpNoDelay;
  }

  @Override
  public NativeChannelConfig setTcpNoDelay(boolean value) {
    option(1, value ? 1 : 0);
    tcpNoDelay = value;
    return this;
  }

  @Override
  public boolean isKeepAlive() {
    return keepAlive;
  }

  @Override
  public NativeChannelConfig setKeepAlive(boolean value) {
    option(2, value ? 1 : 0);
    keepAlive = value;
    return this;
  }

  @Override
  public boolean isReuseAddress() {
    return reuseAddress;
  }

  @Override
  public NativeChannelConfig setReuseAddress(boolean value) {
    option(5, value ? 1 : 0);
    reuseAddress = value;
    return this;
  }

  @Override
  public int getReceiveBufferSize() {
    return receiveBuffer;
  }

  @Override
  public NativeChannelConfig setReceiveBufferSize(int value) {
    if (value <= 0) throw new IllegalArgumentException("Receive buffer must be positive");
    option(3, value);
    receiveBuffer = value;
    return this;
  }

  @Override
  public int getSendBufferSize() {
    return sendBuffer;
  }

  @Override
  public NativeChannelConfig setSendBufferSize(int value) {
    if (value <= 0) throw new IllegalArgumentException("Send buffer must be positive");
    option(4, value);
    sendBuffer = value;
    return this;
  }

  @Override
  public int getBacklog() {
    return backlog;
  }

  @Override
  public NativeChannelConfig setBacklog(int value) {
    if (value < 0 || nativeChannel.isActive())
      throw new IllegalArgumentException("Set nonnegative backlog before bind");
    backlog = value;
    return this;
  }

  @Override
  public boolean isAllowHalfClosure() {
    return halfClosure;
  }

  @Override
  public NativeChannelConfig setAllowHalfClosure(boolean value) {
    halfClosure = value;
    return this;
  }

  @Override
  public int getSoLinger() {
    return -1;
  }

  @Override
  public NativeChannelConfig setSoLinger(int value) {
    throw new UnsupportedOperationException("SO_LINGER");
  }

  @Override
  public int getTrafficClass() {
    return 0;
  }

  @Override
  public NativeChannelConfig setTrafficClass(int value) {
    throw new UnsupportedOperationException("IP_TOS");
  }

  @Override
  public NativeChannelConfig setPerformancePreferences(
      int connectionTime, int latency, int bandwidth) {
    throw new UnsupportedOperationException("Performance preferences");
  }

  @Override
  public Map<ChannelOption<?>, Object> getOptions() {
    return getOptions(
        super.getOptions(),
        ChannelOption.TCP_NODELAY,
        ChannelOption.SO_KEEPALIVE,
        ChannelOption.SO_REUSEADDR,
        ChannelOption.SO_RCVBUF,
        ChannelOption.SO_SNDBUF,
        ChannelOption.SO_BACKLOG,
        ChannelOption.ALLOW_HALF_CLOSURE);
  }

  @Override
  @SuppressWarnings({"unchecked", "ReferenceEquality"})
  // Netty ChannelOption constants are identity tokens.

  public <T> T getOption(ChannelOption<T> option) {
    if (option == ChannelOption.TCP_NODELAY) return (T) Boolean.valueOf(isTcpNoDelay());
    if (option == ChannelOption.SO_KEEPALIVE) return (T) Boolean.valueOf(isKeepAlive());
    if (option == ChannelOption.SO_REUSEADDR) return (T) Boolean.valueOf(isReuseAddress());
    if (option == ChannelOption.SO_RCVBUF) return (T) Integer.valueOf(getReceiveBufferSize());
    if (option == ChannelOption.SO_SNDBUF) return (T) Integer.valueOf(getSendBufferSize());
    if (option == ChannelOption.SO_BACKLOG) return (T) Integer.valueOf(getBacklog());
    if (option == ChannelOption.ALLOW_HALF_CLOSURE)
      return (T) Boolean.valueOf(isAllowHalfClosure());
    return super.getOption(option);
  }

  @Override
  // Netty ChannelOption constants are identity tokens.
  @SuppressWarnings("ReferenceEquality")
  public <T> boolean setOption(ChannelOption<T> option, T value) {
    validate(option, value);
    if (option == ChannelOption.TCP_NODELAY) setTcpNoDelay((Boolean) value);
    else if (option == ChannelOption.SO_KEEPALIVE) setKeepAlive((Boolean) value);
    else if (option == ChannelOption.SO_REUSEADDR) setReuseAddress((Boolean) value);
    else if (option == ChannelOption.SO_RCVBUF) setReceiveBufferSize((Integer) value);
    else if (option == ChannelOption.SO_SNDBUF) setSendBufferSize((Integer) value);
    else if (option == ChannelOption.SO_BACKLOG) setBacklog((Integer) value);
    else if (option == ChannelOption.ALLOW_HALF_CLOSURE) setAllowHalfClosure((Boolean) value);
    else return super.setOption(option, value);
    return true;
  }

  @Override
  public NativeChannelConfig setConnectTimeoutMillis(int value) {
    super.setConnectTimeoutMillis(value);
    return this;
  }

  @Override
  public NativeChannelConfig setMaxMessagesPerRead(int value) {
    super.setMaxMessagesPerRead(value);
    return this;
  }

  @Override
  public NativeChannelConfig setWriteSpinCount(int value) {
    super.setWriteSpinCount(value);
    return this;
  }

  @Override
  public NativeChannelConfig setAllocator(ByteBufAllocator value) {
    allocatorSet = true;
    super.setAllocator(value);
    return this;
  }

  @Override
  public NativeChannelConfig setRecvByteBufAllocator(RecvByteBufAllocator value) {
    receiveAllocatorSet = true;
    super.setRecvByteBufAllocator(value);
    return this;
  }

  @Override
  public NativeChannelConfig setAutoRead(boolean value) {
    super.setAutoRead(value);
    return this;
  }

  @Override
  public NativeChannelConfig setAutoClose(boolean value) {
    super.setAutoClose(value);
    return this;
  }

  @Override
  public NativeChannelConfig setMessageSizeEstimator(MessageSizeEstimator value) {
    super.setMessageSizeEstimator(value);
    return this;
  }

  @Override
  public NativeChannelConfig setWriteBufferWaterMark(WriteBufferWaterMark value) {
    super.setWriteBufferWaterMark(value);
    return this;
  }

  @Override
  public NativeChannelConfig setWriteBufferHighWaterMark(int value) {
    super.setWriteBufferHighWaterMark(value);
    return this;
  }

  @Override
  public NativeChannelConfig setWriteBufferLowWaterMark(int value) {
    super.setWriteBufferLowWaterMark(value);
    return this;
  }
}
