import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.TransportNative;
import dev.elide.bemo.transport.tls.NativeSslContext;
import dev.elide.bemo.transport.tls.NativeSslContextBuilder;
import io.netty.buffer.UnpooledByteBufAllocator;
import io.netty.util.ReferenceCountUtil;
import java.nio.ByteBuffer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLEngineResult;
import javax.net.ssl.SSLException;

/** Immutable TLS policy contract, without sockets or a transport driver. */
public final class NativeSslPolicyTest {

  public static void main(String[] args) throws Exception {
    verify(
        new FfmTransportNative(Path.of(args[0])),
        Files.readAllBytes(Path.of(args[1])),
        Files.readAllBytes(Path.of(args[2])));
  }

  private static void require(boolean value, String message) {
    if (!value) throw new AssertionError(message);
  }

  private static void invalid(Runnable action) {
    try {
      action.run();
      throw new AssertionError("invalid TLS policy accepted");
    } catch (IllegalArgumentException expected) {
    }
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    long owner = api.ownerNew(4 * 1024 * 1024);
    require(owner != 0, "TLS owner");
    try {
      String[][] suites = {
        {"TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256"},
        {"TLS_AES_256_GCM_SHA384"},
        {"TLS_AES_128_GCM_SHA256", "TLS_AES_256_GCM_SHA384"},
      };
      for (int index = 0; index < suites.length; index++) {
        String version = index == 0 ? "TLSv1.2" : "TLSv1.3";
        NativeSslContext clientContext =
            NativeSslContextBuilder.forClient(api)
                .workload(owner)
                .trustAnchors(cert)
                .protocols(version)
                .cipherSuites(suites[index])
                .applicationProtocols("h2")
                .build();
        NativeSslContext serverContext =
            NativeSslContextBuilder.forServer(api, cert, key)
                .workload(owner)
                .protocols(version)
                .cipherSuites(suites[index])
                .applicationProtocols("h2")
                .build();
        SSLEngine client =
            clientContext.newEngine(UnpooledByteBufAllocator.DEFAULT, "localhost", 443);
        SSLEngine server = serverContext.newEngine(UnpooledByteBufAllocator.DEFAULT);
        try {
          require(
              clientContext.cipherSuites().equals(List.of(suites[index])), "context cipher order");
          require(Arrays.equals(client.getEnabledCipherSuites(), suites[index]), "enabled ciphers");
          require(
              Arrays.equals(client.getEnabledProtocols(), new String[] {version}),
              "enabled version");
          invalid(() -> client.setEnabledProtocols(new String[] {"TLSv1.2", "TLSv1.3"}));
          client.setEnabledProtocols(new String[] {version});
          client.setEnabledCipherSuites(suites[index]);
          if (suites[index].length > 1) {
            String[] reversed = {suites[index][1], suites[index][0]};
            invalid(() -> client.setEnabledCipherSuites(reversed));
          }
          require(!client.getSession().isValid(), "unestablished client session");
          require(!server.getSession().isValid(), "unestablished server session");
          handshake(client, server);
          require(client.getSession().isValid(), "established client session");
          require(server.getSession().isValid(), "established server session");
          require(version.equals(client.getSession().getProtocol()), "negotiated version");
          require(
              suites[index][0].equals(client.getSession().getCipherSuite()),
              "negotiated cipher preference");
          require("h2".equals(client.getApplicationProtocol()), "policy prefix preserves ALPN");
        } finally {
          ReferenceCountUtil.release(client);
          ReferenceCountUtil.release(server);
          clientContext.release();
          serverContext.release();
        }
      }
      rejectPeerPolicy(api, owner, cert, key, "TLSv1.2", "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256");
      rejectPeerPolicy(api, owner, cert, key, "TLSv1.3", "TLS_AES_256_GCM_SHA384");
      invalid(() -> NativeSslContextBuilder.forClient(api).protocols());
      invalid(() -> NativeSslContextBuilder.forClient(api).protocols("TLSv1.1"));
      invalid(() -> NativeSslContextBuilder.forClient(api).protocols("TLSv1.3", "TLSv1.3"));
      invalid(() -> NativeSslContextBuilder.forClient(api).cipherSuites());
      invalid(() -> NativeSslContextBuilder.forClient(api).cipherSuites("TLS_FAKE"));
      invalid(
          () ->
              NativeSslContextBuilder.forClient(api)
                  .workload(owner)
                  .trustAnchors(cert)
                  .protocols("TLSv1.2")
                  .cipherSuites("TLS_AES_128_GCM_SHA256")
                  .build());
      require(api.ownerUsed(owner) == 0, "engine storage returned after policy checks");
    } finally {
      require(api.ownerRelease(owner) == 0, "TLS owner release");
    }
    System.out.println("Native TLS protocol and cipher policy checks passed (no sockets)");
  }

  private static void rejectPeerPolicy(
      TransportNative api,
      long owner,
      byte[] cert,
      byte[] key,
      String serverVersion,
      String serverCipher)
      throws Exception {
    NativeSslContext clientContext =
        NativeSslContextBuilder.forClient(api)
            .workload(owner)
            .trustAnchors(cert)
            .protocols("TLSv1.3")
            .cipherSuites("TLS_AES_128_GCM_SHA256")
            .build();
    NativeSslContext serverContext =
        NativeSslContextBuilder.forServer(api, cert, key)
            .workload(owner)
            .protocols(serverVersion)
            .cipherSuites(serverCipher)
            .build();
    SSLEngine client = clientContext.newEngine(UnpooledByteBufAllocator.DEFAULT, "localhost", 443);
    SSLEngine server = serverContext.newEngine(UnpooledByteBufAllocator.DEFAULT);
    try {
      try {
        handshake(client, server);
        throw new AssertionError("incompatible peer policy silently widened");
      } catch (SSLException expected) {
        require(!client.getSession().isValid(), "unestablished client session is invalid");
        require(!server.getSession().isValid(), "failed server session is invalid");
      }
    } finally {
      ReferenceCountUtil.release(client);
      ReferenceCountUtil.release(server);
      clientContext.release();
      serverContext.release();
    }
    require(api.ownerUsed(owner) == 0, "failed policy handshake returned engine storage");
  }

  private static void handshake(SSLEngine client, SSLEngine server) throws Exception {
    ByteBuffer empty = ByteBuffer.allocate(0);
    ByteBuffer toServer = ByteBuffer.allocateDirect(65536);
    ByteBuffer toClient = ByteBuffer.allocateDirect(65536);
    client.beginHandshake();
    server.beginHandshake();
    for (int round = 0; round < 64; round++) {
      client.wrap(empty, toServer);
      unwrap(server, toServer);
      server.wrap(empty, toClient);
      unwrap(client, toClient);
      if (client.getHandshakeStatus() == SSLEngineResult.HandshakeStatus.NOT_HANDSHAKING
          && server.getHandshakeStatus() == SSLEngineResult.HandshakeStatus.NOT_HANDSHAKING) return;
    }
    throw new AssertionError("TLS policy handshake stalled");
  }

  private static void unwrap(SSLEngine engine, ByteBuffer wire) throws Exception {
    wire.flip();
    ByteBuffer sink = ByteBuffer.allocateDirect(65536);
    while (wire.hasRemaining()) {
      SSLEngineResult result = engine.unwrap(wire, sink);
      if (result.bytesConsumed() == 0 && result.bytesProduced() == 0) break;
    }
    wire.compact();
  }
}
