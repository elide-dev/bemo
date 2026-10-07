package dev.elide.bemo.examples;

import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.svm.CapiTransportNative;
import org.graalvm.nativeimage.ImageInfo;

/** Use JVM FFM or statically linked C imports without changing the HTTP application. */
public final class BemoRuntime {
  private BemoRuntime() {}

  public static TransportNative binding() {
    return ImageInfo.inImageCode() ? new CapiTransportNative() : new FfmTransportNative();
  }

  public static TransportNative create() {
    boolean nativeImage = ImageInfo.inImageCode();
    System.out.println(
        "Bemo transport enabled (" + (nativeImage ? "CAPI" : "FFM") + ", AUTO driver)");
    return binding();
  }
}
