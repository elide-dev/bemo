package dev.elide.bemo;

/** Shared boundary for the FFM and Native Image adapters. */
public interface TransportNative {
  /** Current Bemo ABI, independent of Elide transport ABI 3. */
  int ABI_VERSION = 1;

  /** The complete Elide transport ABI 3 boundary is available. */
  long CAP_TRANSPORT_V3 = 1;

  /** {@code bemo_alloc} and {@code bemo_free} allocate from the transport's own allocator. */
  long CAP_NATIVE_ALLOC = 2;

  /**
   * Queries the native ABI.
   *
   * @return the loaded native library's ABI version
   */
  int abiVersion();

  /**
   * Queries available data-plane operations.
   *
   * @return implemented capabilities; unknown bits must be ignored
   */
  long capabilities();

  /** Rejects a native library whose ABI cannot be used by these bindings. */
  default void requireCompatible() {
    int actual = abiVersion();
    if (actual != ABI_VERSION) {
      throw new LinkageError("Bemo ABI mismatch: expected " + ABI_VERSION + ", got " + actual);
    }
  }
}
