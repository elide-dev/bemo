import dev.elide.dokar.transport.*;
import io.netty.channel.AdaptiveRecvByteBufAllocator;
import io.netty.channel.RecvByteBufAllocator;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.ConcurrentLinkedQueue;

/** Every wire completion must update adaptation before a TLS pump or user callback resubmits. */
public final class NativeReceiveAllocatorTest {

  private static final class CheckedHandle extends RecvByteBufAllocator.DelegatingHandle {

    private int attempted;
    private boolean pending;
    private boolean reported;
    private int completed;
    private int largest;
    private String failure;

    private AssertionError failure(String message) {
      failure = message;
      return new AssertionError(message);
    }

    CheckedHandle() {
      super(new AdaptiveRecvByteBufAllocator(64, 64, 65536).newHandle());
    }

    @Override
    public int guess() {
      if (pending) throw failure("receive resubmitted before allocator feedback");
      attempted = super.guess();
      largest = Math.max(largest, attempted);
      pending = true;
      return attempted;
    }

    @Override
    public void attemptedBytesRead(int value) {
      if (!pending || value != attempted) throw failure("lost submitted receive capacity");
      super.attemptedBytesRead(value);
    }

    @Override
    public void lastBytesRead(int value) {
      if (value < 0 || value > attempted) throw failure("invalid receive byte count");
      reported = true;
      super.lastBytesRead(value);
    }

    @Override
    public void readComplete() {
      if (!pending || !reported) throw failure("incomplete allocator feedback");
      pending = false;
      reported = false;
      completed++;
      super.readComplete();
    }
  }

  public static void main(String[] args) throws Exception {
    verify(
        new BackendTransport(new FfmTransportNative(Path.of(args[0]))),
        Files.readAllBytes(Path.of(args[1])),
        Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    ConcurrentLinkedQueue<CheckedHandle> handles = new ConcurrentLinkedQueue<>();
    try {
      NativeTransferTest.verify(
          api,
          cert,
          key,
          () -> {
            CheckedHandle handle = new CheckedHandle();
            handles.add(handle);
            return handle;
          });
    } catch (Throwable error) {
      for (CheckedHandle handle : handles) {
        if (handle.failure != null) throw new AssertionError(handle.failure, error);
      }
      throw error;
    }
    if (handles.size() != 4)
      throw new AssertionError("expected TCP and TLS client/server allocators");
    for (CheckedHandle handle : handles) {
      if (handle.completed < 2 || handle.largest <= 64)
        throw new AssertionError("receive allocator never adapted");
    }
    System.out.println("TCP/TLS wire receive allocator feedback and growth checks passed");
  }
}
