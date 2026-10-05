package dev.elide.bemo;

import dev.elide.bemo.ffm.FfmTransportNative;
import java.nio.file.Path;

/** Exercises dynamic loading on a plain JVM, with no Elide runtime on the classpath. */
public final class FfmContract {
  public static void main(String[] args) throws Exception {
    for (String forbidden :
        new String[] {"dev.elide.runtime.Unsafe", "dev.elide.bemo.svm.CapiTransportNative"}) {
      try {
        Class.forName(forbidden);
        throw new AssertionError("Unexpected runtime dependency: " + forbidden);
      } catch (ClassNotFoundException expected) {
        // The JVM test deliberately includes only API, FFM, and contract classes.
      }
    }
    FfmTransportNative transport = new FfmTransportNative(Path.of(args[0]));
    try (transport) {
      Contract.verify(transport);
    }
    try {
      transport.abiVersion();
      throw new AssertionError("Closed binding was callable");
    } catch (IllegalStateException expected) {
      // The arena protects unloaded library addresses.
    }
    try (var ignored = new FfmTransportNative(Path.of(args[0] + ".missing"))) {
      throw new AssertionError("Missing library was accepted: " + ignored.abiVersion());
    } catch (IllegalArgumentException | UnsatisfiedLinkError expected) {
      // Loading a missing library must fail immediately.
    }
    if (args.length > 1) {
      try (var incompatible = new FfmTransportNative(Path.of(args[1]))) {
        throw new AssertionError("Incompatible library was accepted: " + incompatible.abiVersion());
      } catch (LinkageError expected) {
        if (!expected.getMessage().contains("ABI mismatch")) {
          throw new AssertionError("Expected version negotiation failure", expected);
        }
      }
    }
    System.out.println("FFM contract passed");
  }
}
