/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.netty.v2.tls;

import dev.elide.netty.v2.TransportNative;
import io.netty.buffer.ByteBufAllocator;
import io.netty.handler.ssl.ApplicationProtocolNegotiator;
import io.netty.handler.ssl.SslContext;
import io.netty.util.AbstractReferenceCounted;
import io.netty.util.ReferenceCounted;
import java.io.ByteArrayInputStream;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.Collections;
import java.util.Enumeration;
import java.util.List;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSessionContext;

/**
 * Netty {@link SslContext} over an immutable Rustls configuration; {@link #newHandler} yields a
 * stock {@code SslHandler} over {@link NativeSslEngine}. Engines retain the context, so releasing
 * it frees native configuration only after its last engine is released. Build with {@link
 * NativeSslContextBuilder}.
 */
public final class NativeSslContext extends SslContext implements ReferenceCounted {
  final TransportNative api;
  private final long workload;
  private final boolean client;
  private final List<String> protocols;
  private final List<String> enabledProtocols;
  private final List<String> cipherSuites;
  private final byte[] localChain;
  private final SessionContext sessions = new SessionContext();
  private final AbstractReferenceCounted references =
      new AbstractReferenceCounted() {
        @Override
        protected void deallocate() {
          free();
        }

        @Override
        public ReferenceCounted touch(Object hint) {
          return this;
        }
      };
  private long handle;
  private X509Certificate[] localCertificates;

  NativeSslContext(
      TransportNative api,
      long workload,
      long handle,
      boolean client,
      List<String> protocols,
      List<String> enabledProtocols,
      List<String> cipherSuites,
      byte[] localChain,
      boolean startTls) {
    super(startTls);
    this.api = api;
    this.workload = workload;
    this.handle = handle;
    this.client = client;
    this.protocols = List.copyOf(protocols);
    this.enabledProtocols = List.copyOf(enabledProtocols);
    this.cipherSuites = List.copyOf(cipherSuites);
    this.localChain = localChain;
  }

  /** Native engine for {@code name} (clients) or null (servers); zero when released or invalid. */
  synchronized long newEngine(byte[] name) {
    return handle == 0 ? 0 : api.engineNew(workload, handle, name);
  }

  List<String> enabledProtocols() {
    return enabledProtocols;
  }

  List<String> applicationProtocols() {
    return protocols;
  }

  /** Configured local identity chain, parsed on first use. */
  synchronized X509Certificate[] localCertificates() {
    if (localChain == null) return null;
    if (localCertificates == null) {
      try {
        localCertificates =
            CertificateFactory.getInstance("X.509")
                .generateCertificates(new ByteArrayInputStream(localChain))
                .stream()
                .map(X509Certificate.class::cast)
                .toArray(X509Certificate[]::new);
      } catch (CertificateException error) {
        localCertificates = new X509Certificate[0];
      }
    }
    return localCertificates.length == 0 ? null : localCertificates;
  }

  private synchronized void free() {
    if (handle != 0) api.engineContextRelease(handle);
    handle = 0;
  }

  @Override
  public boolean isClient() {
    return client;
  }

  @Override
  public List<String> cipherSuites() {
    return cipherSuites;
  }

  @Override
  @SuppressWarnings("deprecation")
  public ApplicationProtocolNegotiator applicationProtocolNegotiator() {
    return () -> protocols;
  }

  @Override
  public SSLEngine newEngine(ByteBufAllocator alloc) {
    return new NativeSslEngine(this, null, -1);
  }

  @Override
  public SSLEngine newEngine(ByteBufAllocator alloc, String peerHost, int peerPort) {
    return new NativeSslEngine(this, peerHost, peerPort);
  }

  @Override
  public SSLSessionContext sessionContext() {
    return sessions;
  }

  @Override
  public int refCnt() {
    return references.refCnt();
  }

  @Override
  public ReferenceCounted retain() {
    references.retain();
    return this;
  }

  @Override
  public ReferenceCounted retain(int increment) {
    references.retain(increment);
    return this;
  }

  @Override
  public ReferenceCounted touch() {
    return this;
  }

  @Override
  public ReferenceCounted touch(Object hint) {
    return this;
  }

  @Override
  public boolean release() {
    return references.release();
  }

  @Override
  public boolean release(int decrement) {
    return references.release(decrement);
  }

  /** Rustls keeps resumption state natively; sessions are not enumerable from Java. */
  private static final class SessionContext implements SSLSessionContext {
    private volatile int timeout = 86400;
    private volatile int size;

    @Override
    public SSLSession getSession(byte[] sessionId) {
      return null;
    }

    @Override
    public Enumeration<byte[]> getIds() {
      return Collections.emptyEnumeration();
    }

    @Override
    public void setSessionTimeout(int seconds) {
      if (seconds < 0) throw new IllegalArgumentException("Negative session timeout");
      timeout = seconds;
    }

    @Override
    public int getSessionTimeout() {
      return timeout;
    }

    @Override
    public void setSessionCacheSize(int size) {
      if (size < 0) throw new IllegalArgumentException("Negative session cache size");
      this.size = size;
    }

    @Override
    public int getSessionCacheSize() {
      return size;
    }
  }
}
