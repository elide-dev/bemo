package dev.elide.bemo.examples;

import dev.elide.bemo.transport.TransportNative;
import io.netty.util.concurrent.FastThreadLocal;
import io.netty.util.concurrent.FastThreadLocalThread;
import java.util.Arrays;
import java.util.zip.CRC32;
import java.util.zip.Deflater;

/** Reusable owner-thread gzip state; each request performs a fresh compression operation. */
public final class BenchmarkCompression {
  private static final FastThreadLocal<State> STATES =
      new FastThreadLocal<>() {
        @Override
        protected State initialValue() {
          return create();
        }

        @Override
        protected void onRemoval(State state) {
          state.close();
        }
      };

  private BenchmarkCompression() {}

  private static State create() {
    int level = Integer.getInteger("bemo.gzip.level", 1);
    if (level < 0 || level > 9) throw new IllegalArgumentException("bemo.gzip.level must be 0..9");
    return BenchmarkPayload.bemoEnabled() ? new NativeState(level) : new JdkState(level);
  }

  public static String provider() {
    return BenchmarkPayload.bemoEnabled() ? "bemo-zlib-rs" : "jdk-zlib";
  }

  /** Run on each worker before a framework stops its event-loop registry. */
  public static void releaseCurrentThread() {
    STATES.remove();
  }

  public static byte[] gzip() {
    // Netty removes fast thread locals on event-loop thread exit. A foreign executor
    // has no such guarantee, so use a scoped compressor there instead.
    if (FastThreadLocalThread.currentThreadWillCleanupFastThreadLocals())
      return STATES.get().compress();
    try (State state = create()) {
      return state.compress();
    }
  }

  private interface State extends AutoCloseable {
    byte[] compress();

    @Override
    void close();
  }

  private static final class NativeState implements State {
    private final TransportNative api = BemoRuntime.binding();
    private long owner;
    private long encoder;
    private long input;

    NativeState(int level) {
      try {
        owner = api.ownerNew(8 * 1024 * 1024);
        require(owner != 0, "gzip owner admission");
        encoder = api.gzipNew(owner, level);
        require(encoder != 0, "gzip encoder admission");
        byte[] body = BenchmarkPayload.body();
        input = api.bufferNew(owner, body.length);
        require(input != 0, "gzip input admission");
        api.bufferView(input).put(body);
        require(api.bufferFreeze(input, body.length) == 0, "gzip input freeze");
        System.out.println("Bemo compression enabled (zlib-rs, level " + level + ")");
      } catch (RuntimeException | Error failure) {
        close();
        throw failure;
      }
    }

    @Override
    public byte[] compress() {
      long output = api.gzipCompress(owner, encoder, input);
      require(output != 0, "gzip output admission");
      try {
        var view = api.bufferView(output);
        byte[] bytes = new byte[view.remaining()];
        view.get(bytes);
        return bytes;
      } finally {
        require(api.bufferRelease(output) == 0, "gzip output release");
      }
    }

    @Override
    public void close() {
      if (input != 0) {
        require(api.bufferRelease(input) == 0, "gzip input release");
        input = 0;
      }
      if (encoder != 0) {
        require(api.gzipRelease(encoder) == 0, "gzip encoder release");
        encoder = 0;
      }
      if (owner != 0) {
        require(api.ownerUsed(owner) == 0, "gzip allocation reclamation");
        require(api.ownerRelease(owner) == 0, "gzip owner release");
        owner = 0;
        System.out.println("Bemo compression state reclaimed");
      }
    }
  }

  private static final class JdkState implements State {
    private static final byte[] HEADER = {31, (byte) 139, 8, 0, 0, 0, 0, 0, 0, (byte) 255};
    private final Deflater deflater;
    private final CRC32 checksum = new CRC32();
    private byte[] output = new byte[1024];

    JdkState(int level) {
      deflater = new Deflater(level, true);
    }

    @Override
    public byte[] compress() {
      byte[] input = BenchmarkPayload.body();
      deflater.reset();
      checksum.reset();
      checksum.update(input);
      System.arraycopy(HEADER, 0, output, 0, HEADER.length);
      int position = HEADER.length;
      deflater.setInput(input);
      deflater.finish();
      while (!deflater.finished()) {
        grow(position + 1024);
        int count = deflater.deflate(output, position, output.length - position);
        require(count != 0 || deflater.finished(), "deflater progress");
        position += count;
      }
      grow(position + 8);
      littleEndian(position, checksum.getValue());
      littleEndian(position + 4, input.length);
      return Arrays.copyOf(output, position + 8);
    }

    private void grow(int capacity) {
      if (capacity > output.length)
        output = Arrays.copyOf(output, Math.max(capacity, output.length * 2));
    }

    private void littleEndian(int position, long value) {
      for (int i = 0; i < 4; i++) output[position + i] = (byte) (value >>> (8 * i));
    }

    @Override
    public void close() {
      deflater.end();
    }
  }

  private static void require(boolean condition, String message) {
    if (!condition) throw new IllegalStateException(message);
  }
}
