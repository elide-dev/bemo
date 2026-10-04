/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package io.netty.handler.ssl;

import javax.net.ssl.SSLEngine;

/**
 * Exposes Netty's package-private ALPN accessor: {@link SslHandler#applicationProtocol()}, and so
 * {@link ApplicationProtocolNegotiationHandler}, report only engines implementing it. Lives in
 * Netty's package on the class path; a named {@code io.netty.handler} module would reject it.
 */
public abstract class ApplicationProtocolSslEngine extends SSLEngine
    implements ApplicationProtocolAccessor {
  protected ApplicationProtocolSslEngine(String peerHost, int peerPort) {
    super(peerHost, peerPort);
  }

  /** Negotiated ALPN identifier, or null when none was negotiated. */
  @Override
  public abstract String getNegotiatedApplicationProtocol();
}
