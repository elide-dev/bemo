import dev.elide.netty.v2.svm.CapiTransportNative;
import java.nio.file.Files;
import java.nio.file.Path;

public final class CapiSslPolicyTest {

  public static void main(String[] args) throws Exception {
    NativeSslPolicyTest.verify(
        new CapiTransportNative(),
        Files.readAllBytes(Path.of(args[0])),
        Files.readAllBytes(Path.of(args[1])));
  }
}
