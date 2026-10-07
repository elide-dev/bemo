package dev.elide.bemo.svm;

import dev.elide.bemo.TransportNative;
import dev.elide.bemo.svm.generated.BemoNatives;
import java.util.List;
import org.graalvm.nativeimage.c.CContext;
import org.graalvm.nativeimage.c.function.CLibrary;

/** Native Image binding to the same ABI, linked from Cargo's static library. */
@CContext(CapiTransportNative.Headers.class)
@CLibrary(value = "bemo_ffi", requireStatic = true)
public final class CapiTransportNative implements TransportNative {
  /** Creates a binding and validates the statically linked ABI. */
  public CapiTransportNative() {
    requireCompatible();
  }

  /** Header configuration evaluated by the Native Image builder. */
  public static final class Headers implements CContext.Directives {
    /** Creates the header configuration. */
    public Headers() {}

    @Override
    public List<String> getHeaderFiles() {
      return List.of("<bemo.h>");
    }
  }

  private static int nativeAbiVersion() {
    return BemoNatives.bemo_abi_version();
  }

  private static long nativeCapabilities() {
    return BemoNatives.bemo_capabilities();
  }

  @Override
  public int abiVersion() {
    return nativeAbiVersion();
  }

  @Override
  public long capabilities() {
    return nativeCapabilities();
  }
}
