/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.dokar.transport;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.UnpooledDirectByteBuf;
import io.netty.util.internal.CleanableDirectBuffer;
import java.nio.ByteBuffer;
import org.jspecify.annotations.Nullable;

/** Netty reference counting owns the native handle; all storage is freed by Rust. */
final class NativeByteBuf extends UnpooledDirectByteBuf {
  private final NativeByteBufAllocator allocator;
  private @Nullable Allocation initial;
  private Allocation current;
  private boolean frozen;

  NativeByteBuf(NativeByteBufAllocator allocator, int capacity, int maximum) {
    this(allocator, Allocation.create(allocator, capacity), maximum, 0);
  }

  NativeByteBuf(NativeByteBufAllocator allocator, long handle, int length) {
    this(allocator, Allocation.received(allocator.api, handle, length), Integer.MAX_VALUE, length);
  }

  NativeByteBuf(NativeByteBufAllocator allocator, long handle) {
    this(allocator, new Allocation(allocator.api, handle));
  }

  private NativeByteBuf(NativeByteBufAllocator allocator, Allocation allocation) {
    this(allocator, allocation, Integer.MAX_VALUE, allocation.buffer().remaining());
  }

  private NativeByteBuf(
      NativeByteBufAllocator allocator, Allocation allocation, int maximum, int length) {
    super(allocator, allocation.buffer(), maximum);
    this.allocator = allocator;
    this.initial = allocation;
    this.current = allocation;
    setIndex(0, length);
  }

  long freeze() {
    ensureAccessible();
    if (!frozen) {
      if (allocator.api.bufferFreeze(current.handle, writerIndex()) != 0) {
        throw new IllegalStateException("Cannot freeze native buffer");
      }
      frozen = true;
    }
    return current.handle;
  }

  // Native handles belong to this binding instance, regardless of value equality.
  @SuppressWarnings("ReferenceEquality")
  boolean belongsTo(TransportNative api) {
    return allocator.api == api;
  }

  @Override
  protected CleanableDirectBuffer allocateDirectBuffer(int capacity) {
    current = Allocation.create(allocator, capacity);
    return current;
  }

  @Override
  public ByteBuf capacity(int capacity) {
    ensureAccessible();
    if (capacity == capacity()) return this;
    if (frozen) throw new IllegalStateException("Submitted native buffer cannot be resized");
    Allocation previous = current;
    try {
      super.capacity(capacity);
    } catch (RuntimeException | Error error) {
      if (current != previous) current.clean();
      current = previous;
      throw error;
    }
    // The superclass treats the initial externally supplied buffer as borrowed.
    if (initial != null) {
      initial.clean();
      initial = null;
    }
    return this;
  }

  @Override
  protected void deallocate() {
    try {
      super.deallocate();
    } finally {
      if (initial != null) {
        initial.clean();
        initial = null;
      }
    }
  }

  private static final class Allocation implements CleanableDirectBuffer {
    private final TransportNative api;
    private long handle;
    private final ByteBuffer bytes;

    Allocation(TransportNative api, long handle) {
      this.api = api;
      this.handle = handle;
      try {
        bytes = api.bufferView(handle).order(java.nio.ByteOrder.BIG_ENDIAN);
      } catch (RuntimeException | Error error) {
        api.bufferRelease(handle);
        throw error;
      }
    }

    static Allocation received(TransportNative api, long handle, int length) {
      Allocation allocation = new Allocation(api, handle);
      try {
        allocation.bytes.limit(length);
        return allocation;
      } catch (RuntimeException | Error error) {
        allocation.clean();
        throw error;
      }
    }

    static Allocation create(NativeByteBufAllocator allocator, int capacity) {
      long handle = allocator.api.bufferNew(allocator.owner, Math.max(1, capacity));
      if (handle == 0) throw new OutOfMemoryError("Native transport allocation budget exhausted");
      Allocation allocation = new Allocation(allocator.api, handle);
      allocation.bytes.limit(capacity);
      return allocation;
    }

    @Override
    public ByteBuffer buffer() {
      return bytes;
    }

    @Override
    public void clean() {
      if (handle != 0) {
        api.bufferRelease(handle);
        handle = 0;
      }
    }
  }
}
