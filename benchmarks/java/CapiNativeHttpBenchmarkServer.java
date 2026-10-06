import dev.elide.bemo.transport.svm.CapiTransportNative;

/** Optimized Native Image entry point using static C bindings, native HTTP, and Rustls. */
public final class CapiNativeHttpBenchmarkServer {
  public static void main(String[] args) throws Exception {
    NativeHttpBenchmarkServer.run(new CapiTransportNative(), args);
  }
}
