/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import java.net.SocketAddress;
import java.net.UnixDomainSocketAddress;
import org.jspecify.annotations.Nullable;

/** Native Unix byte-stream client; descriptor passing and local bind are not supported. */
@SuppressWarnings("deprecation")
public final class NativeDomainSocketChannel extends NativeStreamChannel {
  private @Nullable UnixDomainSocketAddress destination;

  public NativeDomainSocketChannel() {}

  @Override
  long endpoint(SocketAddress address) {
    destination = (UnixDomainSocketAddress) address;
    return io().unixEndpoint(destination);
  }

  @Override
  void refreshAddresses() {
    local = UnixDomainSocketAddress.of("");
    remote = destination;
    invalidateLocalAddress();
    invalidateRemoteAddress();
  }

  @Override
  public UnixDomainSocketAddress localAddress() {
    return (UnixDomainSocketAddress) super.localAddress();
  }

  @Override
  public UnixDomainSocketAddress remoteAddress() {
    return (UnixDomainSocketAddress) super.remoteAddress();
  }
}
