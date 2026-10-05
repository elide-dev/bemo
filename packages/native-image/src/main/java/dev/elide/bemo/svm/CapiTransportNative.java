package dev.elide.bemo.svm;

import dev.elide.bemo.TransportNative;
import java.util.List;
import org.graalvm.nativeimage.c.CContext;
import org.graalvm.nativeimage.c.function.CFunction;
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

  @CFunction("bemo_abi_version")
  private static native int nativeAbiVersion();

  @CFunction("bemo_capabilities")
  private static native long nativeCapabilities();

  @Override
  public int abiVersion() {
    return nativeAbiVersion();
  }

  @Override
  public long capabilities() {
    return nativeCapabilities();
  }
}
