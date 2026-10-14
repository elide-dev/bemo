/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import org.jspecify.annotations.Nullable;

/** Optional backend discovered without adding runtime dependencies to bemo-api. */
public interface TransportNativeProvider {
  /** Higher priority selects an embedding's direct backend before portable FFM. */
  int priority();

  /**
   * Opens an available backend after validating both metadata and transport ABI versions. Returns
   * null when the embedding is absent; a linkage refusal permits the next provider.
   */
  @Nullable TransportNative open();
}
