/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.netty.v2;

import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelOutboundBuffer;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/** TLS record progression belongs to the transport, below the application's Netty pipeline. */
final class NativeTlsSession implements AutoCloseable {
  private final NativeStreamChannel channel;
  private final TransportNative api;
  private final long session;
  private final long descriptor;
  private final ByteBuffer result;
  private io.netty.channel.ChannelPromise closePromise;
  private io.netty.util.concurrent.ScheduledFuture<?> closeTimeout;
  private boolean closeRecord;
  private boolean outputOnly;
  private final java.util.ArrayDeque<ByteBuf> pendingReads = new java.util.ArrayDeque<>();
  private int pendingReadBytes;
  private boolean ready;
  private boolean transmitted;
  private boolean pumping;
  private boolean closed;

  private record QueuedWire(long handle, int accepted, boolean close) {}

  private final java.util.ArrayDeque<QueuedWire> queued = new java.util.ArrayDeque<>();
  private long wire;
  private int wireOffset;
  private int wireLength;
  private int applicationBytes;

  NativeTlsSession(NativeStreamChannel channel, NativeTlsContext context, String peerName) {
    this.channel = channel;
    api = channel.io.api;
    session = context.session(api, channel.workload, channel.io.allocator.owner, peerName);
    descriptor = api.bufferNew(channel.io.allocator.owner, 256);
    if (descriptor == 0) {
      api.tlsRelease(session);
      throw new OutOfMemoryError("TLS descriptor allocation failed");
    }
    result = api.bufferView(descriptor).order(ByteOrder.nativeOrder());
  }

  boolean ready() {
    return ready;
  }

  boolean hasPendingRead() {
    return !pendingReads.isEmpty();
  }

  private void releasePendingReads() {
    for (ByteBuf bytes : pendingReads) bytes.release();
    pendingReads.clear();
    pendingReadBytes = 0;
  }

  void requestOutputShutdown(io.netty.channel.ChannelPromise promise) {
    requestClose(promise, true);
  }

  void requestClose(io.netty.channel.ChannelPromise promise) {
    requestClose(promise, false);
  }

  private void requestClose(io.netty.channel.ChannelPromise promise, boolean halfClose) {
    if (closePromise != null) {
      if (!halfClose) {
        outputOnly = false;
        releasePendingReads();
      }
      closePromise.addListener(
          done -> {
            if (done.isSuccess()) promise.trySuccess();
            else promise.tryFailure(done.cause());
          });
      return;
    }
    outputOnly = halfClose;
    closePromise = promise;
    if (!halfClose) releasePendingReads();
    closeTimeout =
        channel
            .eventLoop()
            .schedule(
                () -> channel.finishTlsClose(promise), 5, java.util.concurrent.TimeUnit.SECONDS);
    pump();
  }

  void feed(NativeIoHandler.Completion event) {
    if (event.result == 0)
      throw new NativeTransportException("TLS peer closed without close_notify");
    if (api.tlsFeed(session, event.value, event.result) != 0)
      throw new NativeTransportException("Native TLS receive failed");
    event.value = 0;
    pump();
  }

  void sent(long bytes) {
    if (bytes <= 0 || bytes > wireLength - wireOffset)
      throw new NativeTransportException("TLS transport write failed");
    wireOffset += (int) bytes;
    if (wireOffset != wireLength) {
      channel.sendTls(wire, wireOffset, wireLength - wireOffset);
      return;
    }
    api.bufferRelease(wire);
    wire = 0;
    if (closeRecord) {
      if (!outputOnly) {
        channel.finishTlsClose(closePromise);
        return;
      }
      closeTimeout.cancel(false);
      io.netty.channel.ChannelPromise promise = closePromise;
      closePromise = null;
      closeRecord = outputOnly = false;
      channel.finishTlsOutputShutdown(promise);
    }
    if (applicationBytes > 0) {
      ChannelOutboundBuffer outbound = channel.unsafe().outboundBuffer();
      if (outbound != null) outbound.removeBytes(applicationBytes);
      applicationBytes = 0;
    }
    if (!queued.isEmpty()) {
      QueuedWire next = queued.removeFirst();
      startWire(next.handle(), next.accepted(), next.close());
      return;
    }
    transmitted = true;
    pump();
  }

  private void startWire(long handle, int accepted, boolean close) {
    wire = handle;
    applicationBytes = accepted;
    closeRecord = close;
    wireOffset = 0;
    wireLength = api.bufferCapacity(wire);
    channel.sendTls(wire, 0, wireLength);
    channel.beginNativeRead();
  }

  void pump() {
    if (pumping || closed || !channel.isActive()) return;
    pumping = true;
    try {
      while (!pendingReads.isEmpty() && (channel.readRequested || channel.settings.isAutoRead())) {
        ByteBuf bytes = pendingReads.removeFirst();
        pendingReadBytes -= bytes.readableBytes();
        channel.deliverTls(bytes);
      }
      for (int transitions = 0; transitions < 64 && !closed && channel.isOpen(); transitions++) {
        int action = transmitted ? 1 : 0;
        long plaintext = 0;
        boolean staged = false;
        int offset = 0;
        int length = 0;
        ChannelOutboundBuffer outbound = channel.unsafe().outboundBuffer();
        if (wire == 0 && closePromise != null && ready && !transmitted) action = 3;
        if (wire == 0 && closePromise == null && ready && !transmitted && outbound != null) {
          ByteBuf bytes = (ByteBuf) outbound.current();
          while (bytes != null && !bytes.isReadable()) {
            outbound.remove();
            bytes = (ByteBuf) outbound.current();
          }
          if (bytes != null) {
            length = Math.min(16384, bytes.readableBytes());
            action = 2;
            if (bytes instanceof NativeByteBuf nativeBytes
                && nativeBytes.belongsTo(api)
                && bytes.refCnt() == 1) {
              plaintext = nativeBytes.freeze();
              offset = bytes.readerIndex();
            } else {
              plaintext = api.bufferNew(channel.io.allocator.owner, length);
              if (plaintext == 0) throw new OutOfMemoryError("TLS plaintext budget exhausted");
              staged = true;
              try {
                bytes.getBytes(bytes.readerIndex(), api.bufferView(plaintext).limit(length));
                if (api.bufferFreeze(plaintext, length) != 0)
                  throw new NativeTransportException("TLS plaintext freeze failed");
                TransportEvents.copy(channel, "tls-write", length);
              } catch (Throwable error) {
                api.bufferRelease(plaintext);
                throw error;
              }
            }
          }
        }
        int status;
        try {
          status = api.tlsStep(session, action, plaintext, offset, length, descriptor);
        } finally {
          if (staged) api.bufferRelease(plaintext);
        }
        if (status != 0)
          throw new NativeTransportException("TLS handshake or record authentication failed");
        transmitted = false;
        long stateWord = result.getLong(0);
        int state = (int) stateWord;
        int accepted = Math.toIntExact(result.getLong(8));
        long encoded = result.getLong(16);
        long received = result.getLong(24);
        if ((stateWord & (1L << 32)) != 0 && !ready) {
          ready = true;
          int size = api.tlsProtocol(session, descriptor);
          if (size < 0) throw new NativeTransportException("TLS ALPN query failed");
          byte[] protocol = new byte[size];
          result.position(0).get(protocol);
          channel.tlsReady(size == 0 ? null : new String(protocol, StandardCharsets.US_ASCII));
        }
        if (closed || !channel.isOpen()) {
          if (received != 0) api.bufferRelease(received);
          if (encoded != 0) api.bufferRelease(encoded);
          return;
        }
        if (received != 0) {
          ByteBuf bytes = channel.io.allocator.received(received);
          TransportEvents.copy(channel, "tls-read", bytes.readableBytes());
          if (channel.readRequested || channel.settings.isAutoRead()) channel.deliverTls(bytes);
          else if (closePromise != null) bytes.release();
          else {
            if (pendingReadBytes + bytes.readableBytes() > 2 * 1024 * 1024) {
              bytes.release();
              throw new NativeTransportException("TLS plaintext read credit exhausted");
            }
            pendingReadBytes += bytes.readableBytes();
            pendingReads.addLast(bytes);
          }
        }
        if (closed || !channel.isOpen()) {
          if (encoded != 0) api.bufferRelease(encoded);
          return;
        }
        if (encoded != 0) {
          if (wire == 0) startWire(encoded, accepted, action == 3);
          else {
            if (queued.size() >= 64) {
              api.bufferRelease(encoded);
              throw new NativeTransportException("TLS flight limit exceeded");
            }
            queued.addLast(new QueuedWire(encoded, accepted, action == 3));
          }
          return;
        }
        if (state == 0) {
          channel.beginNativeRead();
          return;
        }
        if (state == 2) {
          if (wire != 0) {
            channel.beginNativeRead();
            return;
          }
          if (action == 1) continue;
          transmitted = true;
          continue;
        }
        if (state == 5) {
          channel.tlsPeerClosed();
          if (closePromise != null) continue;
          return;
        }
        if (state == 6) {
          channel.finishTlsClose(closePromise == null ? channel.voidPromise() : closePromise);
          return;
        }
        if (state == 3 && length == 0) {
          if (wire == 0 && (closePromise != null || outbound != null && outbound.current() != null))
            continue;
          channel.beginNativeRead();
          return;
        }
      }
      if (!closed && channel.isOpen())
        channel
            .eventLoop()
            .execute(
                () -> {
                  try {
                    pump();
                  } catch (Throwable error) {
                    channel.failTls(error);
                  }
                });
    } finally {
      pumping = false;
    }
  }

  @Override
  public void close() {
    if (closed) return;
    closed = true;
    if (closeTimeout != null) closeTimeout.cancel(false);
    releasePendingReads();
    if (wire != 0) {
      api.bufferRelease(wire);
      wire = 0;
    }
    for (QueuedWire pending : queued) api.bufferRelease(pending.handle());
    queued.clear();
    api.tlsRelease(session);
    api.bufferRelease(descriptor);
  }
}
