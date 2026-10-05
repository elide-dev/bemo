package dev.elide.bemo;

/** Shared assertions run through both binding implementations. */
public final class Contract {
  private Contract() {}

  public static void verify(TransportNative transport) {
    transport.requireCompatible();
    if (transport.abiVersion() != 1
        || transport.capabilities() != TransportNative.CAP_TRANSPORT_V3) {
      throw new AssertionError("Unexpected metadata ABI or transport capabilities");
    }
    TransportNative incompatible =
        new TransportNative() {
          public int abiVersion() {
            return 999;
          }

          public long capabilities() {
            return 0;
          }
        };
    try {
      incompatible.requireCompatible();
      throw new AssertionError("Incompatible ABI was accepted");
    } catch (LinkageError expected) {
      if (!expected.getMessage().contains("999")) {
        throw new AssertionError("ABI diagnostic omitted actual version", expected);
      }
    }
  }
}
