import java.nio.ByteBuffer;
import java.util.Arrays;
import java.util.zip.CRC32;
import java.util.zip.Deflater;

/** Event-thread-confined gzip state; compresses a new independent member on every call. */
final class ReusableGzip implements AutoCloseable {
  private static final byte[] HEADER = {31, (byte) 139, 8, 0, 0, 0, 0, 0, 0, (byte) 255};
  private final Deflater deflater;
  private final CRC32 checksum = new CRC32();
  private byte[] output = new byte[1024];
  private int length;
  private boolean closed;

  ReusableGzip(int level) {
    deflater = new Deflater(level, true);
  }

  int compress(byte[] input) {
    if (closed) throw new IllegalStateException("Compressor is closed");
    length = 0;
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
      if (count == 0 && !deflater.finished()) {
        throw new IllegalStateException("Deflater made no progress");
      }
      position += count;
    }
    grow(position + 8);
    littleEndian(position, checksum.getValue());
    littleEndian(position + 4, input.length);
    length = position + 8;
    return length;
  }

  int put(ByteBuffer target) {
    if (closed || length == 0) throw new IllegalStateException("No compressed member");
    target.put(output, 0, length);
    return length;
  }

  private void grow(int capacity) {
    if (capacity > output.length) {
      output = Arrays.copyOf(output, Math.max(capacity, Math.multiplyExact(output.length, 2)));
    }
  }

  private void littleEndian(int position, long value) {
    for (int i = 0; i < 4; i++) output[position + i] = (byte) (value >>> (8 * i));
  }

  @Override
  public void close() {
    if (!closed) {
      closed = true;
      length = 0;
      deflater.end();
    }
  }
}
