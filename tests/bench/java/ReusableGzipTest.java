import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.util.Arrays;
import java.util.Random;
import java.util.zip.GZIPInputStream;
import java.util.zip.GZIPOutputStream;

/** Checks independent gzip members and reset/teardown on the benchmark's reusable compressor. */
public final class ReusableGzipTest {
  public static void main(String[] args) throws Exception {
    byte[] random = new byte[65536];
    new Random(17).nextBytes(random);
    byte[] repeated = new byte[65536];
    Arrays.fill(repeated, (byte) 'x');
    for (int level : new int[] {1, 6}) {
      ReusableGzip compressor = new ReusableGzip(level);
      try (compressor) {
        for (int round = 0; round < 20; round++) {
          for (byte[] input :
              new byte[][] {random, new byte[0], repeated, new byte[1024], {(byte) round}}) {
            int length = compressor.compress(input);
            ByteBuffer destination = ByteBuffer.allocate(length + 7);
            destination.position(7);
            if (compressor.put(destination) != length || destination.position() != length + 7) {
              throw new AssertionError("Output length/position");
            }
            byte[] encoded = Arrays.copyOfRange(destination.array(), 7, destination.position());
            try (GZIPInputStream decoder = new GZIPInputStream(new ByteArrayInputStream(encoded))) {
              if (!Arrays.equals(input, decoder.readAllBytes()))
                throw new AssertionError("Gzip payload");
            }
            ByteArrayOutputStream expected = new ByteArrayOutputStream();
            try (GZIPOutputStream reference =
                new GZIPOutputStream(expected) {
                  {
                    def.setLevel(level);
                  }
                }) {
              reference.write(input);
            }
            if (!Arrays.equals(encoded, expected.toByteArray())) {
              throw new AssertionError("Selected gzip level wire representation changed");
            }
          }
        }
      }
      compressor.close();
      try {
        compressor.compress(random);
        throw new AssertionError("Compression after close succeeded");
      } catch (IllegalStateException expected) {
        // Released native Deflater state cannot be used again.
      }
    }
    System.out.println("ReusableGzipTest passed");
  }
}
