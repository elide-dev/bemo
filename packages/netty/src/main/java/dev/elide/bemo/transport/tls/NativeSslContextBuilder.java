/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport.tls;

import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.Workload;
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.PrivateKey;
import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.Collection;
import java.util.List;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;
import org.jspecify.annotations.Nullable;

/**
 * Builds a {@link NativeSslContext} from PEM or DER inputs. JDK trust managers are read once as
 * certificate sources; Rustls performs every verification itself, with safe default protocol
 * versions and cipher suites.
 */
public final class NativeSslContextBuilder {
  private final TransportNative api;
  private final boolean client;
  private final ByteArrayOutputStream pem = new ByteArrayOutputStream();
  private final List<byte[]> der = new ArrayList<>();
  private byte @Nullable [] keyPem;
  private byte @Nullable [] keyDer;
  private byte @Nullable [] clientChain;
  private byte @Nullable [] clientIdentities;
  private List<String> protocols = List.of();
  private List<String> tlsProtocols = NativeSslSession.PROTOCOLS;
  private List<String> cipherSuites = NativeSslSession.CIPHER_SUITES;
  private boolean tlsPolicy;
  private boolean startTls;
  private boolean insecureSkipVerify;
  private long workload;

  private NativeSslContextBuilder(TransportNative api, boolean client) {
    this.api = api;
    this.client = client;
  }

  /** Bind this configuration and every engine to the supplied native workload. */
  public NativeSslContextBuilder workload(long workload) {
    if (workload == 0) throw new IllegalArgumentException("Missing native workload");
    this.workload = workload;
    return this;
  }

  /** Client context; supply trust anchors before building. */
  public static NativeSslContextBuilder forClient(TransportNative api) {
    return new NativeSslContextBuilder(api, true);
  }

  /** Client certificate chain and exportable private key, separate from server trust anchors. */
  public NativeSslContextBuilder clientIdentity(byte[] certificateChainPem, byte[] privateKeyPem) {
    requireClient();
    if (clientChain != null || clientIdentities != null)
      throw new IllegalStateException("Client identity already configured");
    clientChain = certificateChainPem.clone();
    keyPem = privateKeyPem.clone();
    return this;
  }

  /** A leaf-first client certificate chain and its exportable private key. */
  public record ClientIdentity(PrivateKey key, List<X509Certificate> chain) {
    public ClientIdentity {
      java.util.Objects.requireNonNull(key);
      chain = List.copyOf(chain);
      if (chain.isEmpty() || chain.size() > 64)
        throw new IllegalArgumentException("Invalid client chain length");
    }
  }

  /** Select the first identity matching the peer's issuer hints and signature schemes. */
  public NativeSslContextBuilder clientIdentities(Collection<ClientIdentity> identities) {
    requireClient();
    if (clientChain != null || clientIdentities != null)
      throw new IllegalStateException("Client identity already configured");
    List<ClientIdentity> selected = List.copyOf(identities);
    if (selected.isEmpty() || selected.size() > 64)
      throw new IllegalArgumentException("Invalid client identity count");
    // ByteArrayOutputStream normally leaves an extra private-key copy in its backing array.
    class SensitiveOutput extends ByteArrayOutputStream {
      private void grow(int extra) {
        int required = Math.addExact(count, extra);
        if (required <= buf.length) return;
        byte[] old = buf;
        buf =
            Arrays.copyOf(
                old, Math.max(required, (int) Math.min(Integer.MAX_VALUE, (long) old.length * 2)));
        Arrays.fill(old, (byte) 0);
      }

      @Override
      public synchronized void write(int value) {
        grow(1);
        super.write(value);
      }

      @Override
      public synchronized void write(byte[] value, int offset, int length) {
        java.util.Objects.checkFromIndexSize(offset, length, value.length);
        grow(length);
        super.write(value, offset, length);
      }

      @Override
      public void close() {
        Arrays.fill(buf, (byte) 0);
      }
    }
    try (SensitiveOutput bytes = new SensitiveOutput();
        DataOutputStream output = new DataOutputStream(bytes)) {
      output.writeByte(1);
      output.writeShort(selected.size());
      for (ClientIdentity identity : selected) {
        ByteArrayOutputStream certificates = new ByteArrayOutputStream();
        for (X509Certificate certificate : identity.chain())
          certificates.writeBytes(certificate.getEncoded());
        output.writeInt(certificates.size());
        certificates.writeTo(output);
        byte @Nullable [] key = identity.key().getEncoded();
        try {
          if (key == null || !"PKCS#8".equals(identity.key().getFormat()))
            throw new IllegalArgumentException("Private key must be PKCS#8 encodable");
          output.writeInt(key.length);
          output.write(key);
        } finally {
          if (key != null) Arrays.fill(key, (byte) 0);
        }
        output.writeShort(identity.chain().size());
        for (X509Certificate certificate : identity.chain()) {
          byte[] issuer = certificate.getIssuerX500Principal().getEncoded();
          if (issuer.length > 65535) throw new IllegalArgumentException("Issuer name too long");
          output.writeShort(issuer.length);
          output.write(issuer);
        }
      }
      clientIdentities = bytes.toByteArray();
      return this;
    } catch (IOException | CertificateEncodingException error) {
      throw new IllegalArgumentException("Unencodable client identity", error);
    }
  }

  /** Client identity from a leaf-first chain and a PKCS#8 encodable private key. */
  public NativeSslContextBuilder clientIdentity(PrivateKey key, X509Certificate... chain) {
    requireClient();
    byte[] encoded = key.getEncoded();
    if (encoded == null || !"PKCS#8".equals(key.getFormat()))
      throw new IllegalArgumentException("Private key must be PKCS#8 encodable");
    byte[] privatePem = null;
    try {
      ByteArrayOutputStream certificates = new ByteArrayOutputStream();
      for (X509Certificate certificate : chain)
        certificates.writeBytes(pem("CERTIFICATE", certificate.getEncoded()));
      privatePem = pem("PRIVATE KEY", encoded);
      return clientIdentity(certificates.toByteArray(), privatePem);
    } catch (CertificateEncodingException error) {
      throw new IllegalArgumentException("Unencodable certificate", error);
    } finally {
      Arrays.fill(encoded, (byte) 0);
      if (privatePem != null) Arrays.fill(privatePem, (byte) 0);
    }
  }

  /** Server context from a PEM certificate chain and PEM private key (PKCS#8, PKCS#1 or SEC1). */
  public static NativeSslContextBuilder forServer(
      TransportNative api, byte[] certificateChainPem, byte[] privateKeyPem) {
    NativeSslContextBuilder builder = new NativeSslContextBuilder(api, false);
    builder.pem.writeBytes(certificateChainPem);
    builder.keyPem = privateKeyPem.clone();
    return builder;
  }

  /** Server context from a certificate chain (leaf first) and its private key. */
  public static NativeSslContextBuilder forServer(
      TransportNative api, PrivateKey key, X509Certificate... chain) {
    NativeSslContextBuilder builder = new NativeSslContextBuilder(api, false);
    builder.certificates(Arrays.asList(chain));
    byte[] encoded = key.getEncoded();
    if (encoded == null || !"PKCS#8".equals(key.getFormat()))
      throw new IllegalArgumentException("Private key must be PKCS#8 encodable");
    builder.keyDer = encoded;
    return builder;
  }

  /** Add PEM trust anchors (clients). */
  public NativeSslContextBuilder trustAnchors(byte @Nullable [] certificatesPem) {
    requireClient();
    pem.writeBytes(certificatesPem);
    pem.write('\n');
    return this;
  }

  /** Add trust anchors (clients). */
  public NativeSslContextBuilder trustAnchors(Collection<? extends X509Certificate> certificates) {
    requireClient();
    certificates(certificates);
    return this;
  }

  /** Add every issuer a JDK trust manager accepts (clients); the manager is not retained. */
  public NativeSslContextBuilder trustManager(TrustManagerFactory factory) {
    requireClient();
    for (TrustManager manager : factory.getTrustManagers())
      if (manager instanceof X509TrustManager x509)
        certificates(Arrays.asList(x509.getAcceptedIssuers()));
    return this;
  }

  /**
   * Explicitly disable client certificate-chain and hostname verification; signatures remain
   * verified.
   */
  public NativeSslContextBuilder insecureSkipVerify(boolean enabled) {
    requireClient();
    insecureSkipVerify = enabled;
    return this;
  }

  /** Supported TLS versions, without initializing a native library or context. */
  public static List<String> supportedProtocols() {
    return NativeSslSession.PROTOCOLS;
  }

  /** Supported IANA cipher suite names in the provider's default preference order. */
  public static List<String> supportedCipherSuites() {
    return NativeSslSession.CIPHER_SUITES;
  }

  /** Enabled TLS versions; only TLSv1.2 and TLSv1.3 are supported. */
  public NativeSslContextBuilder protocols(String... protocols) {
    tlsProtocols = select(protocols, NativeSslSession.PROTOCOLS, "protocol");
    tlsPolicy = true;
    return this;
  }

  /** Enabled IANA cipher suite names in preference order. */
  public NativeSslContextBuilder cipherSuites(String... suites) {
    cipherSuites = select(suites, NativeSslSession.CIPHER_SUITES, "cipher suite");
    tlsPolicy = true;
    return this;
  }

  private static List<String> select(String[] requested, List<String> supported, String kind) {
    if (requested.length == 0)
      throw new IllegalArgumentException("Empty TLS " + kind + " selection");
    List<String> selected = new ArrayList<>();
    for (String value : requested) {
      if (!supported.contains(value) || selected.contains(value))
        throw new IllegalArgumentException("Unsupported or duplicate TLS " + kind + ": " + value);
      selected.add(value);
    }
    return List.copyOf(selected);
  }

  /** ALPN identifiers in preference order. */
  public NativeSslContextBuilder applicationProtocols(String... protocols) {
    for (String protocol : protocols) {
      byte[] bytes = protocol.getBytes(StandardCharsets.US_ASCII);
      if (bytes.length == 0
          || bytes.length > 255
          || !protocol.equals(new String(bytes, StandardCharsets.US_ASCII)))
        throw new IllegalArgumentException("ALPN identifier must contain 1–255 ASCII bytes");
    }
    this.protocols = List.of(protocols);
    return this;
  }

  /** Send the first message in plaintext before TLS begins (Netty StartTLS semantics). */
  public NativeSslContextBuilder startTls(boolean startTls) {
    this.startTls = startTls;
    return this;
  }

  private void requireClient() {
    if (!client) throw new IllegalStateException("Trust anchors apply to client contexts");
  }

  private void certificates(Collection<? extends X509Certificate> certificates) {
    for (X509Certificate certificate : certificates) {
      try {
        der.add(certificate.getEncoded());
      } catch (CertificateEncodingException error) {
        throw new IllegalArgumentException("Unencodable certificate", error);
      }
    }
  }

  private static byte[] pem(String label, byte[] der) {
    return ("-----BEGIN "
            + label
            + "-----\n"
            + Base64.getMimeEncoder(64, new byte[] {'\n'}).encodeToString(der)
            + "\n-----END "
            + label
            + "-----\n")
        .getBytes(StandardCharsets.US_ASCII);
  }

  /**
   * Build the context.
   *
   * @throws IllegalArgumentException when Rustls rejects the certificates, key or protocols
   */
  public NativeSslContext build() {
    boolean derMode = pem.size() == 0 && keyPem == null;
    ByteArrayOutputStream chain = new ByteArrayOutputStream();
    byte @Nullable [] key;
    if (derMode) {
      der.forEach(chain::writeBytes);
      key = keyDer;
    } else {
      chain.writeBytes(pem.toByteArray());
      for (byte[] certificate : der) chain.writeBytes(pem("CERTIFICATE", certificate));
      key = keyPem != null ? keyPem : keyDer == null ? null : pem("PRIVATE KEY", keyDer);
    }
    ByteArrayOutputStream alpn = new ByteArrayOutputStream();
    if (tlsPolicy) {
      alpn.write(1);
      alpn.write(
          (tlsProtocols.contains("TLSv1.2") ? 1 : 0) | (tlsProtocols.contains("TLSv1.3") ? 2 : 0));
      alpn.write(cipherSuites.size());
      for (String suite : cipherSuites) {
        int code = NativeSslSession.suiteCode(suite);
        alpn.write(code >>> 8);
        alpn.write(code);
      }
    }
    for (String protocol : protocols) {
      alpn.write(protocol.length());
      alpn.writeBytes(protocol.getBytes(StandardCharsets.US_ASCII));
    }
    byte @Nullable [] certificates = chain.toByteArray();
    if (clientIdentities != null) key = clientIdentities;
    if (clientChain != null) {
      java.util.Objects.requireNonNull(key, "client identity requires a private key");
      byte[] identity = new byte[clientChain.length + 1 + key.length];
      System.arraycopy(clientChain, 0, identity, 0, clientChain.length);
      identity[clientChain.length] = '\n';
      System.arraycopy(key, 0, identity, clientChain.length + 1, key.length);
      // key can alias keyPem, which is no longer needed after forming the native input.
      Arrays.fill(key, (byte) 0);
      key = identity;
    }
    int flags =
        (client ? 0 : TransportNative.ENGINE_SERVER)
            | (derMode ? TransportNative.ENGINE_DER : 0)
            | (insecureSkipVerify ? TransportNative.ENGINE_INSECURE : 0)
            | (tlsPolicy ? TransportNative.ENGINE_POLICY : 0)
            | (clientChain != null ? TransportNative.ENGINE_CLIENT_IDENTITY : 0)
            | (clientIdentities != null ? TransportNative.ENGINE_CLIENT_IDENTITIES : 0);
    long owner;
    long handle;
    try {
      owner = workload == 0 ? Workload.id(api) : workload;
      handle = api.engineContextNew(owner, flags, certificates, key, alpn.toByteArray());
    } finally {
      if (key != null) Arrays.fill(key, (byte) 0);
      if (keyPem != null) Arrays.fill(keyPem, (byte) 0);
      if (keyDer != null) Arrays.fill(keyDer, (byte) 0);
      if (clientIdentities != null) Arrays.fill(clientIdentities, (byte) 0);
    }
    if (handle == 0) throw new IllegalArgumentException("Invalid native TLS configuration");
    return new NativeSslContext(
        api,
        owner,
        handle,
        client,
        protocols,
        tlsProtocols,
        cipherSuites,
        client ? clientChain : certificates,
        startTls);
  }
}
