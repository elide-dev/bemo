package dev.elide.bemo.transport;

import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;
import org.jspecify.annotations.Nullable;

/** Configuration contract; no native library, framework, or GraalVM SDK is needed. */
public final class TransportNativeSelectionTest {
  private static TransportNative backend(int version) {
    return (TransportNative)
        Proxy.newProxyInstance(
            TransportNative.class.getClassLoader(),
            new Class<?>[] {TransportNative.class},
            (proxy, method, arguments) -> {
              if (method.getName().equals("version")) return version;
              throw new AssertionError("Discovery must not invoke data-plane methods: " + method);
            });
  }

  private static TransportNativeProvider provider(int priority, @Nullable TransportNative backend) {
    return new TransportNativeProvider() {
      @Override
      public int priority() {
        return priority;
      }

      @Override
      public @Nullable TransportNative open() {
        return backend;
      }
    };
  }

  public static void main(String[] args) {
    var direct = backend(3);
    var fallback = backend(3);
    var diagnostics = new ArrayList<String>();
    assert TransportNativeDiscovery.choose(
            List.of(provider(0, fallback), provider(100, direct)), diagnostics::add)
        == direct;
    assert diagnostics.isEmpty();
    assert TransportNativeDiscovery.choose(
            List.of(provider(100, backend(99)), provider(0, fallback)), diagnostics::add)
        == fallback;
    assert diagnostics.size() == 1;
    assert diagnostics.getFirst().contains("ABI mismatch");
    assert TransportNativeDiscovery.choose(
            List.of(provider(100, null), provider(0, fallback)), diagnostics::add)
        == fallback;
    assert diagnostics.size() == 1;
    var mismatchedMetadata =
        new TransportNativeProvider() {
          @Override
          public int priority() {
            return 200;
          }

          @Override
          public TransportNative open() {
            throw new LinkageError("Bemo metadata ABI mismatch");
          }
        };
    assert TransportNativeDiscovery.choose(
            List.of(mismatchedMetadata, provider(0, fallback)), diagnostics::add)
        == fallback;
    assert diagnostics.size() == 2;
    assert TransportNativeDiscovery.choose(List.of(), diagnostics::add) == null;
    assert TransportNative.load() == null; // API-only classpath has no registered provider.
    System.out.println("transport backend selection: OK");
  }
}
