/*
 * Copyright (c) 2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.spring;

import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.http.NativeHttpServer;
import java.nio.charset.StandardCharsets;
import java.security.Key;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.util.Base64;
import java.util.Enumeration;
import java.util.concurrent.TimeUnit;
import org.springframework.boot.ssl.SslBundle;
import org.springframework.boot.web.server.Ssl;
import org.springframework.boot.web.server.WebServer;
import org.springframework.boot.web.server.WebServerException;
import org.springframework.boot.web.server.reactive.AbstractReactiveWebServerFactory;
import org.springframework.http.server.reactive.HttpHandler;

/** Boot factory that preserves the application's HttpHandler and uses Bemo's native HTTP codec. */
public final class BemoReactiveWebServerFactory extends AbstractReactiveWebServerFactory {
  private final TransportNative api;
  private long nativeMemoryLimit = 256L * 1024 * 1024;
  private long requestBodyLimit = 16L * 1024 * 1024;
  private int exchangeLimit = 8192;
  private int nativeWorkers = 1;

  public BemoReactiveWebServerFactory(TransportNative api) {
    this.api = api;
  }

  public void setNativeMemoryLimit(long bytes) {
    nativeMemoryLimit = bytes;
  }

  public void setRequestBodyLimit(long bytes) {
    requestBodyLimit = bytes;
  }

  public void setExchangeLimit(int limit) {
    exchangeLimit = limit;
  }

  public void setNativeWorkers(int workers) {
    if (workers < 1) throw new IllegalArgumentException("native workers");
    nativeWorkers = workers;
  }

  @Override
  public WebServer getWebServer(HttpHandler handler) {
    try {
      String host = getAddress() == null ? "0.0.0.0" : getAddress().getHostAddress();
      Ssl ssl = getSsl();
      boolean encrypted = ssl != null && ssl.isEnabled();
      byte[] certificate = null;
      byte[] key = null;
      if (encrypted) {
        if (ssl.getClientAuth() != null && ssl.getClientAuth() != Ssl.ClientAuth.NONE
            || ssl.getCiphers() != null
            || ssl.getEnabledProtocols() != null
            || !ssl.getServerNameBundles().isEmpty()) {
          throw new IllegalArgumentException(
              "Bemo HTTP TLS currently requires default protocol/cipher policy, one server identity"
                  + " and no client authentication");
        }
        SslBundle bundle = getSslBundle();
        KeyStore store = bundle.getStores().getKeyStore();
        if (store == null) throw new IllegalArgumentException("TLS key store missing");
        String alias = bundle.getKey().getAlias();
        if (alias == null) {
          Enumeration<String> aliases = store.aliases();
          while (aliases.hasMoreElements()) {
            String candidate = aliases.nextElement();
            if (store.isKeyEntry(candidate)) {
              alias = candidate;
              break;
            }
          }
        }
        if (alias == null) throw new IllegalArgumentException("TLS private key missing");
        String password = bundle.getKey().getPassword();
        if (password == null) password = bundle.getStores().getKeyStorePassword();
        Key privateKey = store.getKey(alias, password == null ? null : password.toCharArray());
        Certificate[] chain = store.getCertificateChain(alias);
        if (privateKey == null || chain == null || chain.length == 0)
          throw new IllegalArgumentException("TLS identity incomplete");
        StringBuilder certificates = new StringBuilder();
        for (Certificate entry : chain) certificates.append(pem("CERTIFICATE", entry.getEncoded()));
        certificate = certificates.toString().getBytes(StandardCharsets.US_ASCII);
        key = pem("PRIVATE KEY", privateKey.getEncoded()).getBytes(StandardCharsets.US_ASCII);
      }
      BemoHttpHandlerAdapter adapter =
          new BemoHttpHandlerAdapter(
              handler, (encrypted ? "https" : "http") + "://" + host + ":" + getPort());
      NativeHttpServer server =
          new NativeHttpServer(
              api,
              host,
              getPort(),
              nativeMemoryLimit,
              requestBodyLimit,
              exchangeLimit,
              adapter,
              certificate,
              key,
              nativeWorkers);
      return new WebServer() {
        private int boundPort = -1;

        @Override
        public void start() {
          try {
            boundPort = server.start().toCompletableFuture().get(30, TimeUnit.SECONDS);
            System.out.println(
                "Bemo native HTTP: backend="
                    + api.getClass().getName()
                    + " port="
                    + boundPort
                    + " tls="
                    + encrypted);
          } catch (Exception failure) {
            server.close();
            throw new WebServerException("Bemo HTTP startup failed", failure);
          }
        }

        @Override
        public void stop() {
          server.close();
          try {
            server.stopped().toCompletableFuture().get(1, TimeUnit.SECONDS);
          } catch (Exception failure) {
            throw new WebServerException("Bemo HTTP retirement failed", failure);
          }
        }

        @Override
        public int getPort() {
          return boundPort;
        }
      };
    } catch (Exception failure) {
      throw new WebServerException("Bemo HTTP configuration failed", failure);
    }
  }

  private static String pem(String type, byte[] encoded) {
    return "-----BEGIN "
        + type
        + "-----\n"
        + Base64.getMimeEncoder(64, new byte[] {'\n'}).encodeToString(encoded)
        + "\n-----END "
        + type
        + "-----\n";
  }
}
