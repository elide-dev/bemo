import dev.elide.bemo.transport.svm.CapiTransportNative;

public final class CapiTransportTest {

  public static void main(String[] args) throws Exception {
    TransportAbiTest.verify(new BackendTransport(new CapiTransportNative()));
  }
}
