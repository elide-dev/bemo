import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.NativeDirectByteBufAllocator;
import dev.elide.bemo.transport.TransportNative;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.CompositeByteBuf;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicReference;

/** Netty direct buffers allocated and freed through Bemo's native allocator. */
public final class NativeDirectAllocatorTest {
  public static void main(String[] args) throws Exception {
    verify(new BackendTransport(new FfmTransportNative(Path.of(args[0]))));
  }

  public static void verify(TransportNative api) throws Exception {
    if (!api.supportsNativeAllocation())
      throw new AssertionError("binding does not expose CAP_NATIVE_ALLOC");
    var allocator = new NativeDirectByteBufAllocator(api);
    long baseline = allocator.nativeAllocatedBytes();

    // Raw ABI: aligned, writable, accounted, and invalid sizes allocate nothing.
    long address = api.nativeAllocate(4096);
    if (address == 0 || (address & 15) != 0) throw new AssertionError("unaligned allocation");
    var view = api.nativeView(address, 4096);
    // Netty's NIO-backed buffers (used when Unsafe is unavailable, as in Native Image) need
    // Java's default order; a native-order view silently byte-swaps across reallocation.
    if (view.order() != java.nio.ByteOrder.BIG_ENDIAN)
      throw new AssertionError("native views must be big-endian, got " + view.order());
    view.putLong(4088, 0x0123456789abcdefL);
    if (view.getLong(4088) != 0x0123456789abcdefL) throw new AssertionError("view lost bytes");
    if (api.nativeAllocatedBytes() < baseline + 4096) throw new AssertionError("not accounted");
    api.nativeFree(address, 4096);
    api.nativeFree(0, 4096);
    if (api.nativeAllocate(0) != 0) throw new AssertionError("zero-byte allocation");

    // Growth reallocates through the native allocator and preserves contents.
    ByteBuf buffer = allocator.directBuffer(8, 1 << 20);
    if (!buffer.isDirect()) throw new AssertionError("expected a direct buffer");
    String kind = buffer.getClass().getSimpleName() + (buffer.hasMemoryAddress() ? "/address" : "");
    // Write across several reallocations, checking the whole prefix after each one.
    int capacity = buffer.capacity();
    for (int index = 0; index < 4096; index++) {
      buffer.writeInt(index);
      if (buffer.capacity() != capacity) {
        capacity = buffer.capacity();
        verifyPrefix(buffer, index + 1, kind + " after growth to " + capacity);
      }
    }
    verifyPrefix(buffer, 4096, kind);
    if (allocator.nativeAllocatedBytes() < baseline + 4096 * 4)
      throw new AssertionError("grown buffer is not native");
    // Copies and composites draw from the same allocator.
    ByteBuf copy = buffer.copy(0, 64);
    CompositeByteBuf composite = allocator.compositeDirectBuffer();
    composite.addComponent(true, copy.retainedSlice(0, 32));
    composite.addComponent(true, allocator.directBuffer(16).writeLong(7).writeLong(8));
    if (composite.getLong(32) != 7) throw new AssertionError("composite lost bytes");
    composite.release();
    copy.release();
    buffer.release();

    // Zero-capacity buffers own nothing and can still grow.
    ByteBuf empty = allocator.directBuffer(0);
    empty.writeByte(1);
    empty.release();

    // Buffers allocated on one thread are released on another.
    List<ByteBuf> buffers = new ArrayList<>();
    for (int index = 0; index < 256; index++)
      buffers.add(allocator.directBuffer(1024).writeLong(index));
    var failure = new AtomicReference<Throwable>();
    var done = new CountDownLatch(1);
    Thread release =
        new Thread(
            () -> {
              try {
                for (int index = 0; index < buffers.size(); index++) {
                  if (buffers.get(index).readLong() != index)
                    throw new AssertionError("cross-thread buffer lost bytes");
                  buffers.get(index).release();
                }
              } catch (Throwable error) {
                failure.set(error);
              } finally {
                done.countDown();
              }
            });
    release.start();
    done.await();
    if (failure.get() != null) throw new AssertionError(failure.get());

    if (allocator.nativeAllocatedBytes() != baseline)
      throw new AssertionError(
          "native allocations leaked: " + (allocator.nativeAllocatedBytes() - baseline) + " bytes");
    System.out.println("Netty direct buffers through Bemo's native allocator passed: " + kind);
  }

  private static void verifyPrefix(ByteBuf buffer, int count, String context) {
    for (int index = 0; index < count; index++) {
      int value = buffer.getInt(index * 4);
      if (value != index) {
        var nio = buffer.internalNioBuffer(index * 4, 4);
        throw new AssertionError(
            "growth corrupted payload ("
                + context
                + "): int "
                + index
                + " of "
                + count
                + " reads "
                + value
                + " via ByteBuf, "
                + nio.getInt(nio.position())
                + " via NIO view");
      }
    }
  }
}
