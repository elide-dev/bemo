import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.file.Path;

/** Calls a deliberately defective native library through FFM; the sanitizer must abort it. */
public final class SanitizerCanary {
  public static void main(String[] args) throws Throwable {
    Linker linker = Linker.nativeLinker();
    SymbolLookup symbols = SymbolLookup.libraryLookup(Path.of(args[0]), Arena.global());
    switch (args[1]) {
      case "overflow" -> {
        MethodHandle overflow =
            linker.downcallHandle(
                symbols.find("bemo_canary_overflow").orElseThrow(),
                FunctionDescriptor.of(ValueLayout.JAVA_INT));
        int unused = (int) overflow.invokeExact();
      }
      case "race" -> {
        // Mirror transport calls: JVM-created threads entering instrumented native code.
        MethodHandle race =
            linker.downcallHandle(
                symbols.find("bemo_canary_race").orElseThrow(),
                FunctionDescriptor.ofVoid(ValueLayout.JAVA_INT));
        Thread[] threads = new Thread[4];
        for (int index = 0; index < threads.length; index++) {
          threads[index] =
              Thread.ofPlatform()
                  .start(
                      () -> {
                        try {
                          race.invokeExact(100_000);
                        } catch (Throwable failure) {
                          throw new AssertionError(failure);
                        }
                      });
        }
        for (Thread thread : threads) {
          thread.join();
        }
      }
      default -> throw new IllegalArgumentException(args[1]);
    }
    System.out.println("canary completed without a sanitizer report");
  }
}
