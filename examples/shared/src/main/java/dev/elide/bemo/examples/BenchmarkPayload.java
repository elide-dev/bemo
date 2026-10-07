package dev.elide.bemo.examples;

import java.nio.charset.StandardCharsets;

/** One deterministic, compressible body shared by every large-response workload. */
public final class BenchmarkPayload {
  private static final byte[] BODY = create();

  private BenchmarkPayload() {}

  private static byte[] create() {
    byte[] pattern =
        "Hello, World! Bemo framework benchmark.\n".getBytes(StandardCharsets.US_ASCII);
    byte[] body = new byte[128 * 1024];
    for (int i = 0; i < body.length; i++) body[i] = pattern[i % pattern.length];
    return body;
  }

  /** Borrow this immutable application fixture; HTTP encoders must not modify it. */
  public static byte[] body() {
    return BODY;
  }

  public static boolean bemoEnabled() {
    return Boolean.parseBoolean(System.getProperty("bemo.enabled", "true"));
  }

  /** Explicit gzip refusal takes precedence over a wildcard. */
  public static boolean acceptsGzip(String acceptEncoding) {
    if (acceptEncoding == null) return false;
    double wildcard = 0;
    for (String entry : acceptEncoding.split(",")) {
      String[] parts = entry.trim().split(";");
      double quality = 1;
      for (int i = 1; i < parts.length; i++) {
        String parameter = parts[i].trim();
        if (parameter.regionMatches(true, 0, "q=", 0, 2)) {
          try {
            quality = Double.parseDouble(parameter.substring(2));
          } catch (NumberFormatException invalid) {
            quality = 0;
          }
        }
      }
      if (!(quality >= 0 && quality <= 1)) quality = 0;
      if (parts[0].equalsIgnoreCase("gzip")) return quality > 0;
      if (parts[0].equals("*")) wildcard = quality;
    }
    return wildcard > 0;
  }
}
