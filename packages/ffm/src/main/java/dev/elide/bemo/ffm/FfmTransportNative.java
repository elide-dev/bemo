package dev.elide.bemo.ffm;

import dev.elide.bemo.TransportNative;
import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.file.Path;

/** Stock-JVM binding; requires JDK 22+ and native access for the calling module. */
@SuppressWarnings("restricted")
public final class FfmTransportNative implements TransportNative, AutoCloseable {
  private final Arena arena;
  private final MethodHandle version;
  private final MethodHandle capabilities;

  /**
   * Loads the shared library from the matching native classifier JAR or explicit system property.
   */
  public FfmTransportNative() {
    this(dev.elide.bemo.ffm.NativeLibraryLoader.libraryPath());
  }

  /**
   * Loads an explicit library path, validates its ABI, and retains it until close.
   *
   * @param library native library produced by Cargo for this platform
   */
  public FfmTransportNative(Path library) {
    arena = Arena.ofShared();
    try {
      SymbolLookup symbols = SymbolLookup.libraryLookup(library.toAbsolutePath(), arena);
      Linker linker = Linker.nativeLinker();
      version =
          linker.downcallHandle(
              symbols.find("bemo_abi_version").orElseThrow(),
              FunctionDescriptor.of(ValueLayout.JAVA_INT));
      requireCompatible();
      capabilities =
          linker.downcallHandle(
              symbols.find("bemo_capabilities").orElseThrow(),
              FunctionDescriptor.of(ValueLayout.JAVA_LONG));
    } catch (Throwable failure) {
      arena.close();
      throw failure;
    }
  }

  @Override
  public int abiVersion() {
    try {
      return (int) version.invokeExact();
    } catch (RuntimeException | Error failure) {
      throw failure;
    } catch (Throwable failure) {
      throw new LinkageError("Cannot invoke bemo_abi_version", failure);
    }
  }

  @Override
  public long capabilities() {
    try {
      return (long) capabilities.invokeExact();
    } catch (RuntimeException | Error failure) {
      throw failure;
    } catch (Throwable failure) {
      throw new LinkageError("Cannot invoke bemo_capabilities", failure);
    }
  }

  /** Unloads this binding's library; callers must finish all calls before closing. */
  @Override
  public void close() {
    arena.close();
  }
}
