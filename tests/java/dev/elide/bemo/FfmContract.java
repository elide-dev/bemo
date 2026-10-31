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
    String previous = System.setProperty("bemo.native.path", args[0]);
    try {
      var selected = dev.elide.bemo.transport.TransportNative.load();
      if (!(selected instanceof dev.elide.bemo.transport.FfmTransportNative)) {
        throw new AssertionError("ServiceLoader did not select the portable FFM backend");
      }
      long owner = selected.ownerNew(4096);
      if (owner == 0) throw new AssertionError("Selected backend rejected workload");
      try {
        long buffer = selected.bufferNew(owner, 64);
        if (buffer == 0) throw new AssertionError("Selected backend rejected buffer");
        try {
          selected.bufferView(buffer).put((byte) 42);
          if (selected.bufferFreeze(buffer, 1) != 0) throw new AssertionError("Freeze failed");
          if (selected.bufferView(buffer).get(0) != 42)
            throw new AssertionError("Payload mismatch");
        } finally {
          if (selected.bufferRelease(buffer) != 0) throw new AssertionError("Release failed");
        }
      } finally {
        if (selected.ownerRelease(owner) != 0) throw new AssertionError("Owner release failed");
      }
    } finally {
      if (previous == null) System.clearProperty("bemo.native.path");
      else System.setProperty("bemo.native.path", previous);
    }
    System.out.println("FFM contract passed");
  }
}
