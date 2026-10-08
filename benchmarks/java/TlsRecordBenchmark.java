import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.tls.NativeSslContextBuilder;
import io.netty.buffer.ByteBufAllocator;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.util.ReferenceCountUtil;
import java.io.ByteArrayInputStream;
import java.nio.ByteBuffer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLEngineResult;

/**
 * Established TLS 1.3 encryption; handshake, peer decryption, and validation are outside its timer.
 */
public final class TlsRecordBenchmark {
  private static final String CIPHER = "TLS_AES_128_GCM_SHA256";
  private static final ByteBuffer EMPTY = ByteBuffer.allocateDirect(0);

  private static void tasks(SSLEngine engine) {
    Runnable task;
    while ((task = engine.getDelegatedTask()) != null) task.run();
  }

  private static void pump(
      SSLEngine engine, ByteBuffer incoming, ByteBuffer outgoing, ByteBuffer plain)
      throws Exception {
    tasks(engine);
    incoming.flip();
    while (incoming.hasRemaining()) {
      plain.clear();
      SSLEngineResult result = engine.unwrap(incoming, plain);
      tasks(engine);
      if (result.bytesConsumed() == 0 && result.bytesProduced() == 0) break;
    }
    incoming.compact();
    if (engine.getHandshakeStatus() == SSLEngineResult.HandshakeStatus.NEED_UNWRAP_AGAIN) {
      plain.clear();
      engine.unwrap(EMPTY, plain);
      tasks(engine);
    }
    engine.wrap(EMPTY, outgoing);
  }

  private static void handshake(SSLEngine server, SSLEngine client) throws Exception {
    ByteBuffer toServer = ByteBuffer.allocateDirect(256 * 1024);
    ByteBuffer toClient = ByteBuffer.allocateDirect(256 * 1024);
    ByteBuffer plain = ByteBuffer.allocateDirect(256 * 1024);
    server.beginHandshake();
    client.beginHandshake();
    for (int i = 0; i < 1000; i++) {
      pump(client, toClient, toServer, plain);
      pump(server, toServer, toClient, plain);
      if (server.getHandshakeStatus() == SSLEngineResult.HandshakeStatus.NOT_HANDSHAKING
          && client.getHandshakeStatus() == SSLEngineResult.HandshakeStatus.NOT_HANDSHAKING
          && toServer.position() == 0
          && toClient.position() == 0) {
        if (!server.getSession().getProtocol().equals("TLSv1.3")
            || !server.getSession().getCipherSuite().equals(CIPHER)
            || !client.getSession().getCipherSuite().equals(CIPHER))
          throw new IllegalStateException("Negotiated TLS policy differs");
        return;
      }
    }
    throw new IllegalStateException(
        "TLS handshake did not finish: server="
            + server.getHandshakeStatus()
            + " client="
            + client.getHandshakeStatus()
            + " toServer="
            + toServer.position()
            + " toClient="
            + toClient.position());
  }

  public static void main(String[] args) throws Exception {
    if (args.length != 7)
      throw new IllegalArgumentException("library cert key provider size warmup rounds");
    String provider = args[3];
    int size = Integer.parseInt(args[4]),
        warmup = Integer.parseInt(args[5]),
        rounds = Integer.parseInt(args[6]);
    if (size < 1 || size > 128 * 1024 || warmup < 1 || rounds < 1)
      throw new IllegalArgumentException("Invalid record workload");
    byte[] cert = Files.readAllBytes(Path.of(args[1])), key = Files.readAllBytes(Path.of(args[2]));
    SslContext serverContext;
    if (provider.equals("bemo")) {
      serverContext =
          NativeSslContextBuilder.forServer(new FfmTransportNative(Path.of(args[0])), cert, key)
              .protocols("TLSv1.3")
              .cipherSuites(CIPHER)
              .build();
    } else if (provider.equals("openssl")) {
      OpenSsl.ensureAvailability();
      if (!OpenSsl.versionString().contains("BoringSSL"))
        throw new IllegalStateException("BoringSSL required");
      serverContext =
          SslContextBuilder.forServer(new ByteArrayInputStream(cert), new ByteArrayInputStream(key))
              .sslProvider(SslProvider.OPENSSL_REFCNT)
              .protocols("TLSv1.3")
              .ciphers(List.of(CIPHER))
              .build();
    } else throw new IllegalArgumentException("Unknown provider");
    SslContext clientContext =
        SslContextBuilder.forClient()
            .sslProvider(SslProvider.JDK)
            .trustManager(new ByteArrayInputStream(cert))
            .protocols("TLSv1.3")
            .ciphers(List.of(CIPHER))
            .build();
    SSLEngine server = serverContext.newEngine(ByteBufAllocator.DEFAULT);
    SSLEngine client = clientContext.newEngine(ByteBufAllocator.DEFAULT, "localhost", 443);
    try {
      handshake(server, client);
      ByteBuffer source = ByteBuffer.allocateDirect(size),
          wire = ByteBuffer.allocateDirect(size + 8192);
      ByteBuffer plain = ByteBuffer.allocateDirect(256 * 1024);
      for (int i = 0; i < size; i++) source.put((byte) (i % 251));
      source.flip();
      long elapsed = 0, wraps = 0;
      for (int iteration = 0; iteration < warmup + rounds; iteration++) {
        source.rewind();
        wire.clear();
        long started = System.nanoTime();
        int calls = 0;
        while (source.hasRemaining()) {
          SSLEngineResult result = server.wrap(source, wire);
          calls++;
          if (result.getStatus() != SSLEngineResult.Status.OK || result.bytesConsumed() == 0)
            throw new IllegalStateException("Encryption made no progress: " + result);
        }
        long nanos = System.nanoTime() - started;
        if (iteration >= warmup) {
          elapsed += nanos;
          wraps += calls;
        }
        // Verify every byte with the same external peer, outside the encryption timer.
        wire.flip();
        int received = 0;
        while (wire.hasRemaining()) {
          plain.clear();
          SSLEngineResult result = client.unwrap(wire, plain);
          if (result.bytesConsumed() == 0 && result.bytesProduced() == 0)
            throw new IllegalStateException("Peer decryption made no progress: " + result);
          plain.flip();
          while (plain.hasRemaining()) {
            if (plain.get() != (byte) (received++ % 251))
              throw new AssertionError("TLS payload mismatch");
          }
        }
        if (received != size) throw new AssertionError("TLS payload length mismatch");
      }
      System.out.printf(
          java.util.Locale.ROOT,
          "{\"provider\":\"%s\",\"protocol\":\"TLSv1.3\",\"cipher\":\"%s\",\"bytes\":%d,\"rounds\":%d,\"warmup\":%d,\"encrypt_ns_per_response\":%.3f,\"wraps_per_response\":%.3f}%n",
          provider,
          CIPHER,
          size,
          rounds,
          warmup,
          (double) elapsed / rounds,
          (double) wraps / rounds);
    } finally {
      ReferenceCountUtil.release(server);
      ReferenceCountUtil.release(client);
      ReferenceCountUtil.release(serverContext);
      ReferenceCountUtil.release(clientContext);
    }
  }
}
