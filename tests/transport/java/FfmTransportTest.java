import dev.elide.bemo.transport.FfmTransportNative;
import java.nio.file.Path;

public final class FfmTransportTest {

  public static void main(String[] args) throws Exception {
    TransportAbiTest.verify(new BackendTransport(new FfmTransportNative(Path.of(args[0]))));
  }
}
