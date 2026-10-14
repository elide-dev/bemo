/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.ServiceLoader;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;
import org.jspecify.annotations.Nullable;

/** Backend discovery is configuration work, never part of a request's native crossing. */
final class TransportNativeDiscovery {
  private static final AtomicBoolean REPORTED = new AtomicBoolean();

  private TransportNativeDiscovery() {}

  static @Nullable TransportNative load() {
    return choose(
        ServiceLoader.load(TransportNativeProvider.class, TransportNative.class.getClassLoader()),
        message -> {
          if (REPORTED.compareAndSet(false, true)) System.err.println(message);
        });
  }

  static @Nullable TransportNative choose(
      Iterable<TransportNativeProvider> candidates, Consumer<String> diagnostic) {
    var providers = new ArrayList<TransportNativeProvider>();
    candidates.forEach(providers::add);
    providers.sort(Comparator.comparingInt(TransportNativeProvider::priority).reversed());
    for (var provider : providers) {
      try {
        var backend = provider.open();
        if (backend == null) continue;
        int version = backend.version();
        if (version != TransportNative.ABI_VERSION) {
          throw new LinkageError(
              "transport ABI mismatch: expected "
                  + TransportNative.ABI_VERSION
                  + ", got "
                  + version);
        }
        return backend;
      } catch (LinkageError | UnsupportedOperationException failure) {
        diagnostic.accept(
            "Bemo: "
                + provider.getClass().getName()
                + " unavailable; trying the next backend ("
                + failure.getMessage()
                + ")");
      }
    }
    return null;
  }
}
