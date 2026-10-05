import dev.elide.bemo.transport.FfmTransportNative;
import java.nio.file.Files;
import java.nio.file.Path;

/** Exercises the resource-loaded data plane without an explicit library path. */
public final class AutomaticTransportCheck {
  public static void main(String[] args) throws Exception {
    var api = new BackendTransport(new FfmTransportNative());
    NativeChannelTest.verify(api);
    NativeTlsChannelTest.verify(
        api, Files.readAllBytes(Path.of(args[0])), Files.readAllBytes(Path.of(args[1])));
  }
}
