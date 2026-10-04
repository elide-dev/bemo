package dev.elide.dokar;

/** Shared boundary for the FFM and Native Image adapters. */
public interface TransportNative {
  /** Current Dokar ABI, independent of Elide transport ABI 3. */
  int ABI_VERSION = 1;

  /**
   * Queries the native ABI.
   *
   * @return the loaded native library's ABI version
   */
  int abiVersion();

  /**
   * Queries available data-plane operations.
   *
   * @return implemented transport capabilities; zero means metadata only
   */
  long capabilities();

  /** Rejects a native library whose ABI cannot be used by these bindings. */
  default void requireCompatible() {
    int actual = abiVersion();
    if (actual != ABI_VERSION) {
      throw new LinkageError("Dokar ABI mismatch: expected " + ABI_VERSION + ", got " + actual);
    }
  }
}
