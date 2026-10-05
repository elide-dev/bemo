package dev.elide.bemo.ffm;

import dev.elide.bemo.Contract;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.concurrent.Executors;

/** Runs in fresh JVMs to test selection, extraction, overrides, and duplicate resources. */
public final class NativeLibraryLoaderContract {
  public static void main(String[] args) throws Exception {
    if (!NativeLibraryLoader.classifier("Mac OS X", "arm64").equals("osx-aarch64")
        || !NativeLibraryLoader.classifier("Linux", "amd64").equals("linux-x86_64-gnu")
        || !NativeLibraryLoader.classifier("Windows 11", "AMD64").equals("windows-x86_64")) {
      throw new AssertionError("Platform aliases disagree with packaging");
    }
    try {
      NativeLibraryLoader.classifier("Linux", "x86");
      throw new AssertionError("32-bit platform accepted");
    } catch (UnsatisfiedLinkError expected) {
      // Unsupported platforms must fail before attempting a load.
    }
    if (args.length > 0) {
      try (var transport = new FfmTransportNative()) {
        throw new AssertionError("Expected failure: " + transport.abiVersion());
      } catch (UnsatisfiedLinkError expected) {
        if (!expected.getMessage().contains(args[0])) throw expected;
      }
      return;
    }
    try (var threads = Executors.newFixedThreadPool(8)) {
      var calls = new ArrayList<java.util.concurrent.Future<Path>>();
      for (int i = 0; i < 16; i++) calls.add(threads.submit(NativeLibraryLoader::libraryPath));
      Path library = calls.getFirst().get();
      if (!Files.isRegularFile(library)) throw new AssertionError("Library not extracted");
      for (var call : calls)
        if (!call.get().equals(library)) throw new AssertionError("Concurrent extractions");
      String workdir = System.getProperty("bemo.native.workdir");
      if (workdir != null && !library.startsWith(Path.of(workdir)))
        throw new AssertionError("Wrong workdir");
    }
    try (var second = new FfmTransportNative()) {
      try (var first = new FfmTransportNative()) {
        Contract.verify(first);
      }
      Contract.verify(second);
    }
    try (var reopened = new FfmTransportNative()) {
      Contract.verify(reopened);
    }
    System.out.println("Automatic native resource loading passed");
  }
}
