/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.bemo.transport;

import io.netty.buffer.AbstractByteBufAllocator;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufAllocator;
import io.netty.buffer.UnpooledDirectByteBuf;
import io.netty.buffer.UnpooledUnsafeDirectByteBuf;
import io.netty.util.internal.CleanableDirectBuffer;
import io.netty.util.internal.PlatformDependent;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.concurrent.atomic.AtomicLong;
import org.jspecify.annotations.Nullable;

/**
 * Direct buffers whose memory comes from Bemo's native allocator, the same allocator the transport
 * uses: bundled mimalloc, or the embedding application's mimalloc or Rust allocator. Netty's own
 * allocation path (the JDK, its cleaner, or {@code malloc}) is bypassed for these buffers.
 *
 * <p>Buffers are not pooled in Java; the native allocator already caches and reuses memory per
 * thread. Buffers may be released on any thread. Heap buffers come from {@link
 * ByteBufAllocator#DEFAULT}. Unlike {@link NativeByteBufAllocator}, allocations are not charged to
 * a transport owner and need no handle.
 *
 * <p>Requires a binding whose {@link TransportNative#supportsNativeAllocation()} is true. This
 * allocator relies on Netty 4.2's {@code CleanableDirectBuffer} hook.
 */
public final class NativeDirectByteBufAllocator extends AbstractByteBufAllocator {
  private final TransportNative api;
  private final boolean unsafe = PlatformDependent.hasUnsafe();

  public NativeDirectByteBufAllocator(TransportNative api) {
    super(true);
    if (!api.supportsNativeAllocation())
      throw new UnsupportedOperationException("Bemo native allocation is unavailable");
    this.api = api;
  }

  @Override
  protected ByteBuf newDirectBuffer(int initialCapacity, int maxCapacity) {
    return unsafe
        ? new UnsafeBuffer(this, initialCapacity, maxCapacity)
        : new SafeBuffer(this, initialCapacity, maxCapacity);
  }

  @Override
  protected ByteBuf newHeapBuffer(int initialCapacity, int maxCapacity) {
    return ByteBufAllocator.DEFAULT.heapBuffer(initialCapacity, maxCapacity);
  }

  @Override
  public boolean isDirectBufferPooled() {
    return false;
  }

  /** Bytes currently allocated through Bemo's native allocator, process-wide. */
  public long nativeAllocatedBytes() {
    return api.nativeAllocatedBytes();
  }

  CleanableDirectBuffer allocate(int capacity) {
    if (capacity == 0) return Empty.INSTANCE;
    long address = api.nativeAllocate(capacity);
    if (address == 0)
      throw new OutOfMemoryError("Bemo native allocator could not provide " + capacity + " bytes");
    try {
      // Netty's NIO-backed buffers read and write through this view in big-endian order.
      ByteBuffer view = api.nativeView(address, capacity).order(ByteOrder.BIG_ENDIAN);
      return new Allocation(api, address, capacity, view);
    } catch (RuntimeException | Error error) {
      api.nativeFree(address, capacity);
      throw error;
    }
  }

  /** Native memory with exactly one free, whichever thread releases the buffer. */
  private static final class Allocation implements CleanableDirectBuffer {
    private final TransportNative api;
    private final AtomicLong address;
    private final int capacity;
    private final ByteBuffer buffer;

    Allocation(TransportNative api, long address, int capacity, ByteBuffer buffer) {
      this.api = api;
      this.address = new AtomicLong(address);
      this.capacity = capacity;
      this.buffer = buffer;
    }

    @Override
    public ByteBuffer buffer() {
      return buffer;
    }

    @Override
    public void clean() {
      long released = address.getAndSet(0);
      if (released != 0) api.nativeFree(released, capacity);
    }

    @Override
    public boolean hasMemoryAddress() {
      return true;
    }

    @Override
    public long memoryAddress() {
      return address.get();
    }
  }

  /** Zero-capacity buffers own no native memory. */
  private enum Empty implements CleanableDirectBuffer {
    INSTANCE;

    private static final ByteBuffer BUFFER = ByteBuffer.allocateDirect(0);

    @Override
    public ByteBuffer buffer() {
      return BUFFER.duplicate();
    }

    @Override
    public void clean() {}
  }

  private static final class UnsafeBuffer extends UnpooledUnsafeDirectByteBuf {
    // The superclass treats the initial externally supplied buffer as borrowed and frees it here.
    private @Nullable CleanableDirectBuffer initial;

    UnsafeBuffer(NativeDirectByteBufAllocator allocator, int initialCapacity, int maxCapacity) {
      this(allocator, allocator.allocate(initialCapacity), maxCapacity);
    }

    private UnsafeBuffer(
        NativeDirectByteBufAllocator allocator, CleanableDirectBuffer allocation, int maxCapacity) {
      super(allocator, allocation.buffer(), maxCapacity);
      this.initial = allocation;
      setIndex(0, 0);
    }

    @Override
    protected CleanableDirectBuffer allocateDirectBuffer(int capacity) {
      return ((NativeDirectByteBufAllocator) alloc()).allocate(capacity);
    }

    @Override
    public ByteBuf capacity(int capacity) {
      ensureAccessible();
      if (capacity == capacity()) return this;
      super.capacity(capacity);
      releaseInitial();
      return this;
    }

    @Override
    protected void deallocate() {
      try {
        super.deallocate();
      } finally {
        releaseInitial();
      }
    }

    private void releaseInitial() {
      if (initial != null) {
        initial.clean();
        initial = null;
      }
    }
  }

  private static final class SafeBuffer extends UnpooledDirectByteBuf {
    private @Nullable CleanableDirectBuffer initial;

    SafeBuffer(NativeDirectByteBufAllocator allocator, int initialCapacity, int maxCapacity) {
      this(allocator, allocator.allocate(initialCapacity), maxCapacity);
    }

    private SafeBuffer(
        NativeDirectByteBufAllocator allocator, CleanableDirectBuffer allocation, int maxCapacity) {
      super(allocator, allocation.buffer(), maxCapacity);
      this.initial = allocation;
      setIndex(0, 0);
    }

    @Override
    protected CleanableDirectBuffer allocateDirectBuffer(int capacity) {
      return ((NativeDirectByteBufAllocator) alloc()).allocate(capacity);
    }

    @Override
    public ByteBuf capacity(int capacity) {
      ensureAccessible();
      if (capacity == capacity()) return this;
      super.capacity(capacity);
      releaseInitial();
      return this;
    }

    @Override
    protected void deallocate() {
      try {
        super.deallocate();
      } finally {
        releaseInitial();
      }
    }

    private void releaseInitial() {
      if (initial != null) {
        initial.clean();
        initial = null;
      }
    }
  }
}
