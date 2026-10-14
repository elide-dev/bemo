/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

/** Portable fallback after direct embedding providers have declined or refused the ABI. */
public final class FfmTransportNativeProvider implements TransportNativeProvider {
  /** ServiceLoader constructor; native initialization happens only when selected. */
  public FfmTransportNativeProvider() {}

  @Override
  public int priority() {
    return 0;
  }

  @Override
  public TransportNative open() {
    try (var metadata = new dev.elide.bemo.ffm.FfmTransportNative()) {
      metadata.requireCompatible();
    }
    try {
      return new FfmTransportNative();
    } catch (IllegalArgumentException incompatible) {
      throw new LinkageError(incompatible.getMessage(), incompatible);
    }
  }
}
