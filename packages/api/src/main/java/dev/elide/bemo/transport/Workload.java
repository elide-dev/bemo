/*
 * Copyright (c) 2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */

package dev.elide.bemo.transport;

import java.util.EnumMap;
import java.util.IdentityHashMap;

/**
 * Transport workload tokens. A workload is a native owner handle named by every operation that
 * starts work; closing it cancels that workload's operations and nothing else. Each execution
 * profile maps to one workload per binding, minted at run time on first use.
 */
public final class Workload {
  /** Elide's own networking, then the guest execution profiles in protocol order. */
  public enum Profile {
    RUNTIME,
    LEGACY,
    TRUSTED,
    SANDBOXED,
    ISOLATED
  }

  /** Profile of work that names none: the guest profile used without {@code --sandbox}. */
  public static final Profile DEFAULT = Profile.LEGACY;

  // Operations charge their storage to explicit owners; profile workloads carry no bound yet.
  private static final long LIMIT = Long.MAX_VALUE;

  private static final IdentityHashMap<TransportNative, EnumMap<Profile, Long>> IDS =
      new IdentityHashMap<>();

  private Workload() {}

  /** The {@link #DEFAULT} profile's workload. */
  public static long id(TransportNative api) {
    return id(api, DEFAULT);
  }

  /** The workload of {@code profile} for this binding, minted on first use. */
  public static synchronized long id(TransportNative api, Profile profile) {
    EnumMap<Profile, Long> ids = IDS.computeIfAbsent(api, key -> new EnumMap<>(Profile.class));
    long id = ids.getOrDefault(profile, 0L);
    if (id == 0) {
      id = api.ownerNew(LIMIT);
      if (id == 0) throw new IllegalStateException("Native workload unavailable");
      ids.put(profile, id);
    }
    return id;
  }

  /**
   * Close and release a profile's workload, cancelling its operations; the next {@link #id} mints a
   * fresh one.
   */
  public static synchronized void close(TransportNative api, Profile profile) {
    EnumMap<Profile, Long> ids = IDS.get(api);
    if (ids == null || !ids.containsKey(profile)) return;
    api.ownerRelease(ids.remove(profile));
  }
}
