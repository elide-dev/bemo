/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

import dev.elide.netty.v2.DriverSelection;
import dev.elide.netty.v2.FfmTransportNative;
import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

/** Plain JVM, no Elide bootstrap: a Netty echo through NativeIoHandler maps no Elide runtime. */
public final class StandaloneTransportCheck {

  public static void main(String[] args) throws Throwable {
    try {
      Class.forName("dev.elide.netty.NativeTransport");
      throw new AssertionError("Elide library discovery is on the classpath");
    } catch (ClassNotFoundException expected) {
      // The adapter and its bindings are the only Elide classes present.
    }
    NativeChannelTest.verify(new BackendTransport(new FfmTransportNative(Path.of(args[0]))));
    DriverSelection selection = DriverSelection.observed();
    if (selection == null) throw new AssertionError("event loop recorded no driver selection");
    if (!mapped("dokar_ffi"))
      throw new AssertionError("module enumeration cannot see the transport");
    if (mapped("elideruntime")) throw new AssertionError("elideruntime was loaded");
    System.out.println("Standalone transport check passed: driver " + selection.describe());
  }

  private static boolean mapped(String library) throws Throwable {
    String file = System.mapLibraryName(library);
    String os = System.getProperty("os.name");
    if (os.equals("Linux"))
      return Files.readAllLines(Path.of("/proc/self/maps")).stream()
          .anyMatch(line -> line.endsWith("/" + file));
    Linker linker = Linker.nativeLinker();
    if (os.startsWith("Mac")) {
      SymbolLookup system = linker.defaultLookup();
      MethodHandle count =
          linker.downcallHandle(
              system.find("_dyld_image_count").orElseThrow(),
              FunctionDescriptor.of(ValueLayout.JAVA_INT));
      MethodHandle name =
          linker.downcallHandle(
              system.find("_dyld_get_image_name").orElseThrow(),
              FunctionDescriptor.of(ValueLayout.ADDRESS, ValueLayout.JAVA_INT));
      for (int index = (int) count.invokeExact() - 1; index >= 0; index--) {
        MemorySegment image = (MemorySegment) name.invokeExact(index);
        if (!image.equals(MemorySegment.NULL)
            && image.reinterpret(4096).getString(0).endsWith("/" + file)) return true;
      }
      return false;
    }
    try (Arena arena = Arena.ofConfined()) {
      MethodHandle handle =
          linker.downcallHandle(
              SymbolLookup.libraryLookup("kernel32", arena).find("GetModuleHandleW").orElseThrow(),
              FunctionDescriptor.of(ValueLayout.ADDRESS, ValueLayout.ADDRESS));
      MemorySegment module =
          (MemorySegment) handle.invokeExact(arena.allocateFrom(file, StandardCharsets.UTF_16LE));
      return !module.equals(MemorySegment.NULL);
    }
  }
}
