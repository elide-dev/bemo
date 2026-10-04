import dev.elide.dokar.transport.svm.CapiTransportNative;

public final class CapiTransportTest {

  public static void main(String[] args) throws Exception {
    TransportAbiTest.verify(new BackendTransport(new CapiTransportNative()));
  }
}
