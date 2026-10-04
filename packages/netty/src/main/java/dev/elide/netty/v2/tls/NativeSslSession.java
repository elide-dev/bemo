/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.netty.v2.tls;

import dev.elide.netty.v2.TransportNative;
import java.io.ByteArrayInputStream;
import java.security.Principal;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import javax.net.ssl.SSLPeerUnverifiedException;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSessionBindingEvent;
import javax.net.ssl.SSLSessionBindingListener;
import javax.net.ssl.SSLSessionContext;

/** Negotiated parameters of one engine, read from Rustls once the handshake completes. */
final class NativeSslSession implements SSLSession {
  /** Largest record the engine emits or accepts, matching JSSE so callers size buffers alike. */
  static final int PACKET_BUFFER_SIZE = 16709;

  static final int APPLICATION_BUFFER_SIZE = 16384;

  /** Rustls AWS-LC defaults, in preference order. */
  static final List<String> PROTOCOLS = List.of("TLSv1.3", "TLSv1.2");

  static final List<String> CIPHER_SUITES =
      List.of(
          "TLS_AES_256_GCM_SHA384",
          "TLS_AES_128_GCM_SHA256",
          "TLS_CHACHA20_POLY1305_SHA256",
          "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
          "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
          "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
          "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
          "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
          "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256");

  private static final int[] SUITE_CODES = {
    0x1302, 0x1301, 0x1303, 0xC02C, 0xC02B, 0xCCA9, 0xC030, 0xC02F, 0xCCA8
  };

  private final NativeSslEngine engine;
  private final long created = System.currentTimeMillis();
  private final Map<String, Object> values = new ConcurrentHashMap<>();
  private volatile long accessed = created;
  private volatile boolean valid = true;
  private String protocol;
  private String cipherSuite;
  private String applicationProtocol;
  private byte[] id;
  private X509Certificate[] peer;
  private X509Certificate[] local;

  NativeSslSession(NativeSslEngine engine) {
    this.engine = engine;
  }

  static int suiteCode(String name) {
    int index = CIPHER_SUITES.indexOf(name);
    if (index < 0) throw new IllegalArgumentException("Unsupported TLS cipher suite: " + name);
    return SUITE_CODES[index];
  }

  static String suiteName(int code) {
    for (int index = 0; index < SUITE_CODES.length; index++)
      if (SUITE_CODES[index] == code) return CIPHER_SUITES.get(index);
    return String.format("0x%04X", code);
  }

  static String protocolName(int code) {
    return switch (code) {
      case 0x0304 -> "TLSv1.3";
      case 0x0303 -> "TLSv1.2";
      default -> "NONE";
    };
  }

  /** Capture negotiated details on the engine's lock, once established; the engine may be freed. */
  void established(TransportNative api, long handle) {
    if (protocol != null) return;
    accessed();
    protocol = protocolName(api.engineInfo(handle, TransportNative.ENGINE_INFO_PROTOCOL, 0, null));
    cipherSuite = suiteName(api.engineInfo(handle, TransportNative.ENGINE_INFO_SUITE, 0, null));
    byte[] alpn = NativeSslEngine.info(api, handle, TransportNative.ENGINE_INFO_ALPN, 0);
    applicationProtocol =
        alpn.length == 0 ? null : new String(alpn, java.nio.charset.StandardCharsets.US_ASCII);
    id = NativeSslEngine.info(api, handle, TransportNative.ENGINE_INFO_SESSION_ID, 0);
    int count = api.engineInfo(handle, TransportNative.ENGINE_INFO_PEER_COUNT, 0, null);
    if (count > 0) {
      X509Certificate[] chain = new X509Certificate[count];
      try {
        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        for (int index = 0; index < count; index++) {
          byte[] der = NativeSslEngine.info(api, handle, TransportNative.ENGINE_INFO_PEER, index);
          chain[index] =
              (X509Certificate) factory.generateCertificate(new ByteArrayInputStream(der));
        }
        peer = chain;
      } catch (CertificateException error) {
        peer = null;
      }
    }
    if (engine.context.isClient()) {
      int localCount = api.engineInfo(handle, TransportNative.ENGINE_INFO_LOCAL_COUNT, 0, null);
      if (localCount > 0) {
        try {
          CertificateFactory factory = CertificateFactory.getInstance("X.509");
          local = new X509Certificate[localCount];
          for (int index = 0; index < localCount; index++) {
            local[index] =
                (X509Certificate)
                    factory.generateCertificate(
                        new ByteArrayInputStream(
                            NativeSslEngine.info(
                                api, handle, TransportNative.ENGINE_INFO_LOCAL, index)));
          }
        } catch (CertificateException error) {
          local = null;
        }
      }
    } else local = engine.context.localCertificates();
  }

  String applicationProtocol() {
    synchronized (engine) {
      return applicationProtocol;
    }
  }

  @Override
  public byte[] getId() {
    synchronized (engine) {
      return id == null ? new byte[0] : id.clone();
    }
  }

  @Override
  public SSLSessionContext getSessionContext() {
    return engine.context.sessionContext();
  }

  @Override
  public long getCreationTime() {
    return created;
  }

  @Override
  public long getLastAccessedTime() {
    return accessed;
  }

  /** Records establishment rather than every record, keeping the record path clock-free. */
  void accessed() {
    accessed = System.currentTimeMillis();
  }

  @Override
  public void invalidate() {
    valid = false;
  }

  @Override
  public boolean isValid() {
    synchronized (engine) {
      return valid && protocol != null && !engine.failed();
    }
  }

  @Override
  public void putValue(String name, Object value) {
    Object previous = values.put(name, value);
    if (previous instanceof SSLSessionBindingListener listener)
      listener.valueUnbound(new SSLSessionBindingEvent(this, name));
    if (value instanceof SSLSessionBindingListener listener)
      listener.valueBound(new SSLSessionBindingEvent(this, name));
  }

  @Override
  public Object getValue(String name) {
    return values.get(name);
  }

  @Override
  public void removeValue(String name) {
    Object previous = values.remove(name);
    if (previous instanceof SSLSessionBindingListener listener)
      listener.valueUnbound(new SSLSessionBindingEvent(this, name));
  }

  @Override
  public String[] getValueNames() {
    return values.keySet().toArray(new String[0]);
  }

  @Override
  public Certificate[] getPeerCertificates() throws SSLPeerUnverifiedException {
    synchronized (engine) {
      if (peer == null) throw NativeSslEngine.unverified();
      return peer.clone();
    }
  }

  @Override
  public Certificate[] getLocalCertificates() {
    synchronized (engine) {
      X509Certificate[] local =
          engine.context.isClient() ? this.local : engine.context.localCertificates();
      return local == null ? null : local.clone();
    }
  }

  @Override
  public Principal getPeerPrincipal() throws SSLPeerUnverifiedException {
    synchronized (engine) {
      if (peer == null) throw NativeSslEngine.unverified();
      return peer[0].getSubjectX500Principal();
    }
  }

  @Override
  public Principal getLocalPrincipal() {
    synchronized (engine) {
      X509Certificate[] local =
          engine.context.isClient() ? this.local : engine.context.localCertificates();
      return local == null ? null : local[0].getSubjectX500Principal();
    }
  }

  @Override
  public String getCipherSuite() {
    synchronized (engine) {
      return cipherSuite == null ? "SSL_NULL_WITH_NULL_NULL" : cipherSuite;
    }
  }

  @Override
  public String getProtocol() {
    synchronized (engine) {
      return protocol == null ? "NONE" : protocol;
    }
  }

  @Override
  public String getPeerHost() {
    return engine.getPeerHost();
  }

  @Override
  public int getPeerPort() {
    return engine.getPeerPort();
  }

  @Override
  public int getPacketBufferSize() {
    return PACKET_BUFFER_SIZE;
  }

  @Override
  public int getApplicationBufferSize() {
    return APPLICATION_BUFFER_SIZE;
  }
}
