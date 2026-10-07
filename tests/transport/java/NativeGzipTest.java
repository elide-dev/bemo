import dev.elide.bemo.transport.TransportNative;
import java.io.ByteArrayInputStream;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Random;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.zip.GZIPInputStream;

/** Shared independent decoder, reset, affinity, and lifetime contract for native gzip. */
public final class NativeGzipTest {
  private static void check(boolean valid) {
    if (!valid) throw new AssertionError("Native gzip contract");
  }

  private static void decode(TransportNative api, long output, byte[] input) throws Exception {
    ByteBuffer view = api.bufferView(output);
    check(view.isReadOnly());
    byte[] encoded = new byte[view.remaining()];
    view.get(encoded);
    try (GZIPInputStream decoder = new GZIPInputStream(new ByteArrayInputStream(encoded))) {
      check(Arrays.equals(decoder.readAllBytes(), input));
    }
  }

  public static void verify(TransportNative api) throws Exception {
    for (int level : new int[] {1, 3, 6}) verifyLevel(api, level);
    System.out.println("Native gzip independent-member and ownership checks passed");
  }

  private static void verifyLevel(TransportNative api, int level) throws Exception {
    long owner = api.ownerNew(4 * 1024 * 1024);
    long encoder = api.gzipNew(owner, level);
    check(encoder != 0 && api.gzipNew(owner, 10) == 0 && api.gzipNew(0, 6) == 0);
    byte[] random = new byte[131072];
    new Random(17).nextBytes(random);
    byte[][] inputs = {random, new byte[0], new byte[65536], new byte[131072], {42}, {1, 2, 3}};
    var outputs = new ArrayList<Long>();
    for (byte[] input : inputs) {
      long source = api.bufferNew(owner, Math.max(1, input.length));
      api.bufferView(source).put(input);
      check(api.gzipCompress(owner, encoder, source) == 0);
      check(api.bufferFreeze(source, input.length) == 0);
      check(api.gzipCompress(0, encoder, source) == 0);
      AtomicBoolean rejected = new AtomicBoolean();
      Thread other =
          new Thread(
              () ->
                  rejected.set(
                      api.gzipCompress(owner, encoder, source) == 0
                          && api.gzipRelease(encoder) == -1));
      other.start();
      other.join();
      check(rejected.get());
      long output = api.gzipCompress(owner, encoder, source);
      check(output != 0);
      outputs.add(output);
      check(api.bufferRelease(source) == 0);
      decode(api, output, input);
    }
    long empty = api.bufferNew(owner, 1);
    check(api.bufferFreeze(empty, 0) == 0);
    long denied = api.ownerNew(1);
    long deniedEncoder = api.gzipNew(denied, level);
    check(deniedEncoder != 0 && api.gzipCompress(denied, deniedEncoder, empty) == 0);
    check(api.ownerUsed(denied) == 0);
    check(api.gzipRelease(deniedEncoder) == 0 && api.ownerRelease(denied) == 0);
    check(api.workloadClose(owner) == 0);
    check(api.gzipCompress(owner, encoder, empty) == 0 && api.gzipNew(owner, level) == 0);
    check(api.bufferRelease(empty) == 0);
    check(api.gzipRelease(encoder) == 0 && api.gzipRelease(encoder) == -1);
    for (int i = 0; i < inputs.length; i++) {
      decode(api, outputs.get(i), inputs[i]);
      check(api.bufferRelease(outputs.get(i)) == 0);
    }
    check(api.ownerUsed(owner) == 0 && api.ownerRelease(owner) == 0);
  }
}
