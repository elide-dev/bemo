/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport.bali;

import bali.api.BemoNative;
import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.TransportNativeProvider;
import org.jspecify.annotations.Nullable;

/** Direct Bali backend takes priority; an absent embedding remains a quiet portable fallback. */
public final class BaliTransportNativeProvider implements TransportNativeProvider {
  public BaliTransportNativeProvider() {}

  @Override
  public int priority() {
    return 100;
  }

  @Override
  public @Nullable TransportNative open() {
    try {
      if (!BemoNative.available()) return null;
    } catch (NoClassDefFoundError absent) {
      return null;
    }
    long versions = BemoNative.abiVersion();
    if ((versions >>> 32) != dev.elide.bemo.TransportNative.ABI_VERSION
        || (versions & 0xffffffffL) != TransportNative.ABI_VERSION) {
      throw new LinkageError("Bemo/Bali ABI mismatch: " + Long.toUnsignedString(versions));
    }
    return new BaliTransportNative();
  }
}
