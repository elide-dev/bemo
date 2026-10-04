package dev.elide.netty.v2;

import io.netty.buffer.ByteBuf;
import java.nio.ByteBuffer;

public final class NativeReceivedBufferTest {

  public static void verify(TransportNative api) {
    long owner = api.ownerNew(64);
    NativeByteBufAllocator allocator = new NativeByteBufAllocator(api, owner);
    long handle = api.bufferNew(owner, 64);
    ByteBuffer storage = api.bufferView(handle);
    for (int i = 0; i < 64; i++) storage.put(i, (byte) 0x5a);
    storage.put(0, (byte) 42);
    ByteBuf received = allocator.received(handle, 1);
    if (received.capacity() != 1 || received.readableBytes() != 1) {
      throw new AssertionError("short receive exposed unused allocation capacity");
    }
    if (received.nioBuffer().capacity() != 1 || received.getByte(0) != 42) {
      throw new AssertionError("received view bounds or payload changed");
    }
    try {
      received.getByte(1);
      throw new AssertionError("unused receive tail remained accessible");
    } catch (IndexOutOfBoundsException expected) {
      // Netty bounds must apply even when an older ABI returns full-capacity storage.
    }
    ByteBuf retained = received.retainedSlice();
    received.release();
    if (api.ownerUsed(owner) != 64 || retained.readByte() != 42) {
      throw new AssertionError("retained receive lost its allocation or budget");
    }
    retained.release();
    if (api.ownerUsed(owner) != 0) throw new AssertionError("received storage leaked");
    long invalid = api.bufferNew(owner, 8);
    try {
      allocator.received(invalid, 9);
      throw new AssertionError("oversized receive length accepted");
    } catch (IllegalArgumentException expected) {
      if (api.ownerUsed(owner) != 0) throw new AssertionError("invalid receive leaked storage");
    }
    api.ownerRelease(owner);
  }
}
