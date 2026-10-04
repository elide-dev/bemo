package dev.elide.dokar.ffm;

import java.io.IOException;
import java.io.InputStream;
import java.net.URL;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/** Resolves the platform classifier and extracts its shared library without a Netty dependency. */
public final class NativeLibraryLoader {
  private static Path extracted;

  private NativeLibraryLoader() {}

  /**
   * Returns the native resource classifier for this 64-bit JVM. Linux artifacts target glibc.
   *
   * @return the classifier used by Dokar's native Maven artifacts
   */
  public static String classifier() {
    return classifier(System.getProperty("os.name"), System.getProperty("os.arch"));
  }

  static String classifier(String osName, String architecture) {
    String arch =
        switch (architecture.toLowerCase(Locale.ROOT)) {
          case "amd64", "x86_64", "x64" -> "x86_64";
          case "aarch64", "arm64" -> "aarch64";
          default ->
              throw new UnsatisfiedLinkError("Unsupported Dokar architecture: " + architecture);
        };
    String os = osName.toLowerCase(Locale.ROOT);
    if (os.startsWith("mac") || os.equals("darwin")) return "osx-" + arch;
    if (os.equals("linux")) return "linux-" + arch + "-gnu";
    if (os.startsWith("windows")) return "windows-" + arch;
    throw new UnsatisfiedLinkError("Unsupported Dokar operating system: " + osName);
  }

  /**
   * Resolves an explicit {@code dokar.native.path} override or extracts the matching native
   * resource. Extraction uses a private temporary directory beneath {@code dokar.native.workdir},
   * or the JVM temporary directory. A single extracted path is shared by bindings in this class
   * loader and retained until JVM exit so closing one binding cannot disrupt another.
   *
   * @return an absolute shared-library path
   */
  public static synchronized Path libraryPath() {
    String override = System.getProperty("dokar.native.path");
    if (override != null) {
      Path path = Path.of(override);
      if (!path.isAbsolute() || !Files.isRegularFile(path)) {
        throw new UnsatisfiedLinkError(
            "dokar.native.path must name an existing absolute library path: " + override);
      }
      return path;
    }
    if (extracted != null) return extracted;
    String classifier = classifier();
    String file = System.mapLibraryName("dokar_ffi");
    String resource = "META-INF/native/" + classifier + "/" + file;
    Path directory = null;
    Path library = null;
    try {
      ClassLoader loader = NativeLibraryLoader.class.getClassLoader();
      List<URL> resources =
          Collections.list(
              loader == null
                  ? ClassLoader.getSystemResources(resource)
                  : loader.getResources(resource));
      if (resources.isEmpty()) {
        throw new UnsatisfiedLinkError(
            "Missing "
                + resource
                + "; add dev.elide.dokar:dokar-ffm:<version>:"
                + classifier
                + " to the runtime classpath, or set dokar.native.path");
      }
      byte[] expected = digest(resources.getFirst());
      for (URL candidate : resources) {
        if (!MessageDigest.isEqual(expected, digest(candidate))) {
          throw new UnsatisfiedLinkError(
              "Conflicting Dokar native resources for " + resource + ": " + resources);
        }
      }
      String workdir = System.getProperty("dokar.native.workdir");
      if (workdir == null) {
        directory = Files.createTempDirectory("dokar-");
      } else {
        Path parent = Path.of(workdir).toAbsolutePath();
        Files.createDirectories(parent);
        directory = Files.createTempDirectory(parent, "dokar-");
      }
      // Delete-on-exit is LIFO: remove the library before its private directory.
      directory.toFile().deleteOnExit();
      library = directory.resolve(file);
      try (InputStream input = resources.getFirst().openStream()) {
        Files.copy(input, library, StandardCopyOption.REPLACE_EXISTING);
      }
      library.toFile().deleteOnExit();
      if (!MessageDigest.isEqual(expected, digest(library.toUri().toURL()))) {
        throw new IOException("Native resource changed during extraction: " + resource);
      }
      extracted = library.toAbsolutePath();
      return extracted;
    } catch (IOException failure) {
      if (library != null) {
        try {
          Files.deleteIfExists(library);
        } catch (IOException cleanup) {
          failure.addSuppressed(cleanup);
        }
      }
      if (directory != null) {
        try {
          Files.deleteIfExists(directory);
        } catch (IOException cleanup) {
          failure.addSuppressed(cleanup);
        }
      }
      UnsatisfiedLinkError error =
          new UnsatisfiedLinkError(
              "Cannot extract " + resource + "; set dokar.native.workdir to a writable directory");
      error.initCause(failure);
      throw error;
    }
  }

  private static byte[] digest(URL resource) throws IOException {
    try (InputStream input = resource.openStream()) {
      MessageDigest digest = MessageDigest.getInstance("SHA-256");
      byte[] buffer = new byte[8192];
      for (int count; (count = input.read(buffer)) != -1; ) digest.update(buffer, 0, count);
      return digest.digest();
    } catch (NoSuchAlgorithmException impossible) {
      throw new AssertionError(impossible);
    }
  }
}
