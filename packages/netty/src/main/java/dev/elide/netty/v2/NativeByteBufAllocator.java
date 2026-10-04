/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.netty.v2;

import io.netty.buffer.AbstractByteBufAllocator;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufAllocator;
import java.util.concurrent.atomic.AtomicBoolean;

/** Native allocations charged to an explicit owner; closing the owner prevents new allocations. */
public final class NativeByteBufAllocator extends AbstractByteBufAllocator
    implements AutoCloseable {
  final TransportNative api;
  final long owner;
  private final boolean owned;
  private final AtomicBoolean closed = new AtomicBoolean();

  public NativeByteBufAllocator(TransportNative api, long owner) {
    this(api, owner, false);
  }

  private NativeByteBufAllocator(TransportNative api, long owner, boolean owned) {
    super(true);
    this.api = api;
    this.owner = owner;
    this.owned = owned;
  }

  /** Create a bounded owner independent of any socket driver. */
  public static NativeByteBufAllocator owned(TransportNative api, long limit) {
    if (limit <= 0) throw new IllegalArgumentException("Native allocator limit must be positive");
    long owner = api.ownerNew(limit);
    if (owner == 0) throw new OutOfMemoryError("Cannot create native allocator owner");
    try {
      return new NativeByteBufAllocator(api, owner, true);
    } catch (RuntimeException | Error error) {
      api.ownerRelease(owner);
      throw error;
    }
  }

  /** Stop new direct allocations; retained buffers remain valid. Borrowed owners are not closed. */
  @Override
  public void close() {
    if (closed.compareAndSet(false, true) && owned) api.ownerRelease(owner);
  }

  @Override
  protected ByteBuf newDirectBuffer(int initialCapacity, int maxCapacity) {
    if (closed.get()) throw new OutOfMemoryError("Native allocator is closed");
    return new NativeByteBuf(this, initialCapacity, maxCapacity);
  }

  @Override
  protected ByteBuf newHeapBuffer(int initialCapacity, int maxCapacity) {
    // SslHandler requests a packet-sized heap buffer per Rustls wrap; reuse Netty's heap pool.
    return ByteBufAllocator.DEFAULT.heapBuffer(initialCapacity, maxCapacity);
  }

  @Override
  public boolean isDirectBufferPooled() {
    return false;
  }

  NativeByteBuf received(long handle, int length) {
    return new NativeByteBuf(this, handle, length);
  }

  NativeByteBuf received(long handle) {
    return new NativeByteBuf(this, handle);
  }
}
