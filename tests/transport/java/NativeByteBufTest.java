import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.NativeByteBufAllocator;
import dev.elide.bemo.transport.TransportNative;
import io.netty.buffer.ByteBuf;
import java.nio.file.Path;

public final class NativeByteBufTest {

  public static void main(String[] args) {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(api);
  }

  public static void verify(TransportNative api) {
    dev.elide.bemo.transport.NativeReceivedBufferTest.verify(api);
    long owner = api.ownerNew(4096);
    NativeByteBufAllocator allocator = new NativeByteBufAllocator(api, owner);
    ByteBuf buffer = allocator.directBuffer(4, 64);
    buffer.writeInt(42);
    buffer.writeLong(123);
    if (buffer.getInt(0) != 42 || buffer.getLong(4) != 123)
      throw new AssertionError(
          "growth corrupted payload: " + buffer.getInt(0) + "/" + buffer.getLong(4));
    if (buffer.getByte(3) != 42 || buffer.getByte(11) != 123)
      throw new AssertionError("Netty network byte order changed");
    ByteBuf slice = buffer.retainedSlice(4, 8);
    buffer.release();
    if (slice.readLong() != 123 || api.ownerUsed(owner) == 0)
      throw new AssertionError("retained storage lost");
    slice.release();
    if (api.ownerUsed(owner) != 0) throw new AssertionError("native allocation leaked");
    api.ownerRelease(owner);
    try (NativeByteBufAllocator owned = NativeByteBufAllocator.owned(api, 4096)) {
      ByteBuf retained = owned.directBuffer(32, 64).writeLong(456);
      ByteBuf view = retained.retainedDuplicate();
      retained.release();
      owned.close();
      owned.close();
      if (view.readLong() != 456)
        throw new AssertionError("owner close invalidated retained storage");
      try {
        owned.directBuffer(8);
        throw new AssertionError("closed owner accepted allocation");
      } catch (OutOfMemoryError expected) {
        // Closing an owner stops allocation, not access to retained buffers.
      } finally {
        var failure = new java.util.concurrent.atomic.AtomicReference<Throwable>();
        Thread release =
            new Thread(
                () -> {
                  try {
                    view.release();
                  } catch (Throwable error) {
                    failure.set(error);
                  }
                });
        release.start();
        try {
          release.join();
          if (failure.get() != null) throw new AssertionError(failure.get());
        } catch (InterruptedException error) {
          throw new AssertionError(error);
        }
      }
    }
    NativeAllocatorChannelTest.verify(api);
    System.out.println("Native ByteBuf checks passed");
  }
}
