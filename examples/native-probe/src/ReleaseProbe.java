import dev.elide.bemo.svm.CapiTransportNative;

/** Check the published static archive without building the Rust source. */
public final class ReleaseProbe {
  public static void main(String[] args) {
    var metadata = new CapiTransportNative();
    metadata.requireCompatible();
    var transport = new dev.elide.bemo.transport.svm.CapiTransportNative();
    long owner = transport.ownerNew(1024 * 1024);
    if (owner == 0) throw new AssertionError("owner allocation failed");
    try {
      long buffer = transport.bufferNew(owner, 32);
      if (buffer == 0) throw new AssertionError("buffer allocation failed");
      try {
        transport.bufferView(buffer).put(0, (byte) 42);
        if (transport.bufferFreeze(buffer, 1) != 0) throw new AssertionError("freeze failed");
        if (transport.bufferView(buffer).get(0) != 42) throw new AssertionError("body mismatch");
      } finally {
        transport.bufferRelease(buffer);
      }
      long driver = transport.driverNew(owner, 0, 8);
      if (driver == 0) throw new AssertionError("driver creation failed");
      System.out.println("Published C API backend: " + transport.driverBackend(driver));
      if (transport.driverRelease(driver) != 0) throw new AssertionError("driver release failed");
    } finally {
      if (transport.ownerRelease(owner) != 0) throw new AssertionError("owner release failed");
    }
    System.out.println(
        "Published C API ABI: " + metadata.abiVersion() + "; ownership probe passed");
  }
}
