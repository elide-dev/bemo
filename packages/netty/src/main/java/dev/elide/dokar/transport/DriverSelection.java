/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.dokar.transport;

import io.netty.util.internal.logging.InternalLoggerFactory;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Backend a v2 driver actually runs and, when AUTO fell back from io_uring, the refused setup's
 * error. Backend codes follow the ABI: 0 auto, 1 polling, 2 io_uring, 3 IOCP.
 */
public record DriverSelection(int requested, int actual, String fallback) {
  private static final int REASON_BYTES = 512;
  private static final AtomicBoolean LOGGED = new AtomicBoolean();
  private static volatile DriverSelection observed;

  /** Query a live driver on its owner thread; {@code owner} funds a transient reason buffer. */
  public static DriverSelection of(TransportNative api, long driver, long owner, int requested) {
    return new DriverSelection(requested, api.driverBackend(driver), reason(api, driver, owner));
  }

  /** Create, query and release one driver on the calling thread; null when none can be created. */
  public static DriverSelection probe(TransportNative api, int requested) {
    long owner = api.ownerNew(REASON_BYTES);
    if (owner == 0) return null;
    long driver = 0;
    try {
      driver = api.driverNew(owner, requested, 8);
      return driver == 0 ? null : of(api, driver, owner, requested);
    } finally {
      if (driver != 0) api.driverRelease(driver);
      api.ownerRelease(owner);
    }
  }

  /** Latest selection made by an event loop in this process, or null before any. */
  public static DriverSelection observed() {
    return observed;
  }

  /**
   * Record the driver an event loop or HTTP owner started. Only Linux AUTO running polling can have
   * fallen back, so every other start costs one backend query and no native buffer or allocation.
   */
  public static DriverSelection observe(
      TransportNative api, long driver, long owner, int requested) {
    int actual = api.driverBackend(driver);
    boolean fellBack =
        requested == 0 && actual == 1 && "Linux".equals(System.getProperty("os.name"));
    return record(requested, actual, fellBack ? reason(api, driver, owner) : null);
  }

  /** Publish a selection; the first fallback in the process is logged once. */
  public static DriverSelection record(int requested, int actual, String fallback) {
    DriverSelection last = observed;
    if (fallback == null
        && last != null
        && last.requested == requested
        && last.actual == actual
        && last.fallback == null) return last;
    DriverSelection selection = new DriverSelection(requested, actual, fallback);
    observed = selection;
    if (fallback != null && !LOGGED.getAndSet(true))
      InternalLoggerFactory.getInstance(DriverSelection.class)
          .info(
              "Transport v2 selected {}; io_uring is unavailable: {}",
              selection.driver(),
              fallback);
    return selection;
  }

  /** Native backend name as reported in diagnostics and flight recordings. */
  public static String name(int backend) {
    return switch (backend) {
      case 1 -> {
        String os = System.getProperty("os.name", "");
        yield os.startsWith("Mac") ? "kqueue" : os.equals("Linux") ? "epoll" : "polling";
      }
      case 2 -> "io-uring";
      case 3 -> "iocp";
      default -> "unknown";
    };
  }

  public String driver() {
    return name(actual);
  }

  /** The actual driver, followed by the io_uring error when AUTO fell back. */
  public String describe() {
    return fallback == null ? driver() : driver() + " (AUTO fallback: " + fallback + ")";
  }

  private static String reason(TransportNative api, long driver, long owner) {
    long output = api.bufferNew(owner, REASON_BYTES);
    if (output == 0) return null;
    try {
      int length = api.driverFallback(driver, output);
      if (length <= 0) return null;
      byte[] bytes = new byte[length];
      api.bufferView(output).get(0, bytes);
      return new String(bytes, StandardCharsets.UTF_8);
    } catch (UnsupportedOperationException unavailable) {
      return null;
    } finally {
      api.bufferRelease(output);
    }
  }
}
