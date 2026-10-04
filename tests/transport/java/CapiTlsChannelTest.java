import dev.elide.netty.v2.svm.CapiTransportNative;
import java.nio.file.Files;
import java.nio.file.Path;

public final class CapiTlsChannelTest {

  public static void main(String[] args) throws Exception {
    CapiCallbackTest.verify();
    var api = new BackendTransport(new CapiTransportNative());
    byte[] cert = Files.readAllBytes(Path.of(args[0])), key = Files.readAllBytes(Path.of(args[1]));
    TransportAbiTest.verify(api);
    NativeByteBufTest.verify(api);
    NativeChannelTest.verify(api);
    NativeLifecycleTest.verify(api);
    NativeReentrantCloseTest.verify(api, cert, key);
    NativeTlsChannelTest.verify(api, cert, key);
    NativeTlsNegativeTest.verify(api, cert, key);
    NativeTlsOrderingTest.verify(api, cert, key);
    NativeTransferTest.verify(api, cert, key);
    NativeReceiveAllocatorTest.verify(api, cert, key);
    NativeJfrTest.verify(api, cert, key);
    NativeSslEngineTest.verify(api, cert, key);
    NativeSslPolicyTest.verify(api, cert, key);
    NativeSslInteropTest.verify(api, Path.of(args[0]), Path.of(args[1]));
  }
}
