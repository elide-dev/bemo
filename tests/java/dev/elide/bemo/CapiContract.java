package dev.elide.bemo;

import dev.elide.bemo.svm.CapiTransportNative;

/** Runs as a statically linked Native Image executable. */
public final class CapiContract {
  public static void main(String[] args) {
    Contract.verify(new CapiTransportNative());
    System.out.println("Native Image C API contract passed");
  }
}
