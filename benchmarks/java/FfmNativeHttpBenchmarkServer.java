import dev.elide.bemo.transport.FfmTransportNative;
import java.nio.file.Path;

/** JVM diagnostic for the same native HTTP server used by the Native Image benchmark. */
public final class FfmNativeHttpBenchmarkServer {
  public static void main(String[] args) throws Exception {
    NativeHttpBenchmarkServer.run(new FfmTransportNative(Path.of(args[0])), args);
  }
}
