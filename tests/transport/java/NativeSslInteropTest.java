import dev.elide.netty.v2.*;
import dev.elide.netty.v2.tls.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import io.netty.channel.nio.NioIoHandler;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.ssl.*;
import io.netty.util.CharsetUtil;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.cert.CertificateFactory;
import java.util.Arrays;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManagerFactory;

/** NativeSslEngine against independent TLS stacks: JDK JSSE and the OpenSSL command line. */
public final class NativeSslInteropTest {

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(api, Path.of(args[1]), Path.of(args[2]));
  }

  public static void verify(TransportNative api, Path certPath, Path keyPath) throws Exception {
    byte[] cert = Files.readAllBytes(certPath);
    byte[] key = Files.readAllBytes(keyPath);
    EventLoopGroup v2 =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(api, 0, 128, 32 * 1024 * 1024));
    EventLoopGroup nio = new MultiThreadIoEventLoopGroup(1, NioIoHandler.newFactory());
    NativeSslContext server =
        NativeSslContextBuilder.forServer(api, cert, key)
            .applicationProtocols(
                ApplicationProtocolNames.HTTP_2, ApplicationProtocolNames.HTTP_1_1)
            .build();
    NativeSslContext client =
        NativeSslContextBuilder.forClient(api)
            .trustManager(trust(cert))
            .applicationProtocols(
                ApplicationProtocolNames.HTTP_2, ApplicationProtocolNames.HTTP_1_1)
            .build();
    try {
      jdkClient(v2, server, cert);
      jdkServer(v2, nio, client, cert, key);
      mutualTls(api, v2, nio, certPath, cert, key);
      if (openssl() == null) {
        System.out.println("OpenSSL interop skipped: openssl not found on PATH");
      } else {
        opensslClient(v2, server, certPath);
        opensslServer(v2, client, certPath, keyPath);
      }
    } finally {
      server.release();
      client.release();
      v2.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      nio.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }

  static TrustManagerFactory trust(byte[] cert) throws Exception {
    KeyStore store = KeyStore.getInstance(KeyStore.getDefaultType());
    store.load(null, null);
    store.setCertificateEntry(
        "localhost",
        CertificateFactory.getInstance("X.509")
            .generateCertificate(new ByteArrayInputStream(cert)));
    TrustManagerFactory factory =
        TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
    factory.init(store);
    return factory;
  }

  static Channel echoServer(
      EventLoopGroup group, Class<? extends ServerChannel> type, SslContext context)
      throws Exception {
    return new ServerBootstrap()
        .group(group)
        .channel(type)
        .childHandler(
            new ChannelInitializer<Channel>() {
              @Override
              protected void initChannel(Channel channel) {
                channel
                    .pipeline()
                    .addLast(context.newHandler(channel.alloc()), new NativeSslEngineTest.Echo());
              }
            })
        .bind(new InetSocketAddress("127.0.0.1", 0))
        .sync()
        .channel();
  }

  /** A blocking JSSE socket negotiates h2 and echoes through the native server engine. */
  static void jdkClient(EventLoopGroup group, NativeSslContext context, byte[] cert)
      throws Exception {
    Channel server = echoServer(group, NativeServerSocketChannel.class, context);
    SSLContext jdk = SSLContext.getInstance("TLS");
    jdk.init(null, trust(cert).getTrustManagers(), null);
    for (String version : new String[] {"TLSv1.3", "TLSv1.2"}) {
      try (SSLSocket socket = (SSLSocket) jdk.getSocketFactory().createSocket()) {
        socket.setSoTimeout(10_000);
        socket.connect(server.localAddress(), 10_000);
        var parameters = socket.getSSLParameters();
        parameters.setEndpointIdentificationAlgorithm("HTTPS");
        parameters.setProtocols(new String[] {version});
        parameters.setApplicationProtocols(new String[] {"h2", "http/1.1"});
        parameters.setServerNames(java.util.List.of(new javax.net.ssl.SNIHostName("localhost")));
        socket.setSSLParameters(parameters);
        socket.startHandshake();
        require(
            "h2".equals(socket.getApplicationProtocol()),
            "JSSE ALPN " + socket.getApplicationProtocol());
        require(version.equals(socket.getSession().getProtocol()), "JSSE protocol");
        byte[] payload = new byte[100_000];
        for (int i = 0; i < payload.length; i++) payload[i] = (byte) i;
        socket.getOutputStream().write(payload);
        require(
            Arrays.equals(payload, socket.getInputStream().readNBytes(payload.length)),
            "JSSE echo " + version);
      }
    }
    server.close().syncUninterruptibly();
    System.out.println("JSSE client against NativeSslEngine server passed");
  }

  /** The native client engine against Netty's JDK SslContext (JSSE engine) server. */
  static void jdkServer(
      EventLoopGroup v2, EventLoopGroup nio, NativeSslContext context, byte[] cert, byte[] key)
      throws Exception {
    jdkServer(v2, nio, context, cert, key, null);
  }

  static void jdkServer(
      EventLoopGroup v2,
      EventLoopGroup nio,
      NativeSslContext context,
      byte[] cert,
      byte[] key,
      byte[] clientTrust)
      throws Exception {
    SslContextBuilder builder =
        SslContextBuilder.forServer(new ByteArrayInputStream(cert), new ByteArrayInputStream(key))
            .sslProvider(SslProvider.JDK)
            .applicationProtocolConfig(
                new ApplicationProtocolConfig(
                    ApplicationProtocolConfig.Protocol.ALPN,
                    ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                    ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                    ApplicationProtocolNames.HTTP_2));
    if (clientTrust != null)
      builder.trustManager(new ByteArrayInputStream(clientTrust)).clientAuth(ClientAuth.REQUIRE);
    SslContext jdk = builder.build();
    Channel server = echoServer(nio, NioServerSocketChannel.class, jdk);
    NativeSslEngineTest.Collector collector = new NativeSslEngineTest.Collector(5);
    Channel channel =
        new Bootstrap()
            .group(v2)
            .channel(NativeSocketChannel.class)
            .handler(
                new ChannelInitializer<Channel>() {
                  @Override
                  protected void initChannel(Channel ch) {
                    ch.pipeline()
                        .addLast(context.newHandler(ch.alloc(), "localhost", 443), collector);
                  }
                })
            .connect(server.localAddress())
            .sync()
            .channel();
    try {
      SslHandler ssl = channel.pipeline().get(SslHandler.class);
      ssl.handshakeFuture().sync();
      require(
          "h2".equals(ssl.applicationProtocol()), "JSSE server ALPN " + ssl.applicationProtocol());
      var local = ssl.engine().getSession().getLocalCertificates();
      if (clientTrust == null) require(local == null, "no client certificate was requested");
      else {
        require(local != null && local.length == 1, "selected local certificate count");
        require(
            Arrays.equals(local[0].getEncoded(), certificate(clientTrust).getEncoded()),
            "selected local certificate");
      }
      channel.writeAndFlush(Unpooled.copiedBuffer("hello", CharsetUtil.US_ASCII));
      require(
          "hello"
              .equals(new String(collector.done.get(10, TimeUnit.SECONDS), CharsetUtil.US_ASCII)),
          "JSSE echo");
    } finally {
      channel.close().syncUninterruptibly();
      server.close().syncUninterruptibly();
    }
    System.out.println("NativeSslEngine client against JSSE server passed");
  }

  static java.security.PrivateKey privateKey(byte[] pem) throws Exception {
    byte[] der =
        java.util.Base64.getMimeDecoder()
            .decode(
                new String(pem, java.nio.charset.StandardCharsets.US_ASCII)
                    .replace("-----BEGIN PRIVATE KEY-----", "")
                    .replace("-----END PRIVATE KEY-----", ""));
    try {
      return java.security.KeyFactory.getInstance("RSA")
          .generatePrivate(new java.security.spec.PKCS8EncodedKeySpec(der));
    } finally {
      Arrays.fill(der, (byte) 0);
    }
  }

  static java.security.cert.X509Certificate certificate(byte[] pem) throws Exception {
    return (java.security.cert.X509Certificate)
        CertificateFactory.getInstance("X.509").generateCertificate(new ByteArrayInputStream(pem));
  }

  static void mutualTls(
      TransportNative api,
      EventLoopGroup v2,
      EventLoopGroup nio,
      Path certPath,
      byte[] cert,
      byte[] key)
      throws Exception {
    byte[] clientCert = Files.readAllBytes(certPath.resolveSibling("client-cert.pem"));
    byte[] clientKey = Files.readAllBytes(certPath.resolveSibling("client-key.pem"));
    for (String version : new String[] {"TLSv1.2", "TLSv1.3"}) {
      for (String mode : new String[] {"pem", "key", "selected"}) {
        NativeSslContextBuilder builder =
            NativeSslContextBuilder.forClient(api)
                .trustAnchors(cert)
                .protocols(version)
                .applicationProtocols("h2");
        if (mode.equals("pem")) builder.clientIdentity(clientCert, clientKey);
        else if (mode.equals("key"))
          builder.clientIdentity(privateKey(clientKey), certificate(clientCert));
        else
          builder.clientIdentities(
              java.util.List.of(
                  new NativeSslContextBuilder.ClientIdentity(
                      privateKey(key), java.util.List.of(certificate(cert))),
                  new NativeSslContextBuilder.ClientIdentity(
                      privateKey(clientKey), java.util.List.of(certificate(clientCert)))));
        NativeSslContext context = builder.build();
        try {
          jdkServer(v2, nio, context, cert, key, clientCert);
        } finally {
          context.release();
        }
      }
    }
    System.out.println(
        "NativeSslEngine client identity selection against required JSSE mutual TLS passed");
  }

  static String openssl() {
    for (String directory : System.getenv().getOrDefault("PATH", "").split(File.pathSeparator)) {
      File candidate = new File(directory, "openssl");
      if (candidate.canExecute()) return candidate.getPath();
    }
    return null;
  }

  /** `openssl s_client` negotiates h2 with the native server and reads back its echo. */
  static void opensslClient(EventLoopGroup group, NativeSslContext context, Path cert)
      throws Exception {
    Channel server = echoServer(group, NativeServerSocketChannel.class, context);
    int port = ((InetSocketAddress) server.localAddress()).getPort();
    for (String version : new String[] {"-tls1_3", "-tls1_2"}) {
      Process process =
          new ProcessBuilder(
                  openssl(),
                  "s_client",
                  "-connect",
                  "127.0.0.1:" + port,
                  "-servername",
                  "localhost",
                  "-verify_hostname",
                  "localhost",
                  "-CAfile",
                  cert.toString(),
                  "-verify_return_error",
                  "-alpn",
                  "h2,http/1.1",
                  version,
                  "-ign_eof")
              .redirectErrorStream(true)
              .start();
      try (OutputStream input = process.getOutputStream()) {
        input.write("ping-native\n".getBytes(CharsetUtil.US_ASCII));
        input.flush();
        String output = readUntil(process.getInputStream(), "ping-native\n", "ALPN protocol: h2");
        require(output.contains("Verify return code: 0 (ok)"), "s_client verification:\n" + output);
      } finally {
        process.destroy();
        process.waitFor(5, TimeUnit.SECONDS);
      }
    }
    server.close().syncUninterruptibly();
    System.out.println("OpenSSL s_client against NativeSslEngine server passed");
  }

  /** The native client verifies `openssl s_server -rev` and reads its reversed reply. */
  static void opensslServer(EventLoopGroup group, NativeSslContext context, Path cert, Path key)
      throws Exception {
    int port;
    try (ServerSocket probe = new ServerSocket(0)) {
      port = probe.getLocalPort();
    }
    Process process =
        new ProcessBuilder(
                openssl(),
                "s_server",
                "-accept",
                "127.0.0.1:" + port,
                "-cert",
                cert.toString(),
                "-key",
                key.toString(),
                "-alpn",
                "h2",
                "-rev")
            .redirectErrorStream(true)
            .redirectOutput(ProcessBuilder.Redirect.DISCARD)
            .start();
    try {
      long deadline;
      NativeSslEngineTest.Collector collector = null;
      Channel channel = null;
      deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(10);
      while (channel == null) {
        NativeSslEngineTest.Collector attempt =
            (collector = new NativeSslEngineTest.Collector("evitan-olleh\n".length()));
        ChannelFuture connect =
            new Bootstrap()
                .group(group)
                .channel(NativeSocketChannel.class)
                .handler(
                    new ChannelInitializer<Channel>() {
                      @Override
                      protected void initChannel(Channel ch) {
                        ch.pipeline()
                            .addLast(context.newHandler(ch.alloc(), "localhost", port), attempt);
                      }
                    })
                .connect(new InetSocketAddress("127.0.0.1", port))
                .awaitUninterruptibly();
        if (connect.isSuccess()) channel = connect.channel();
        else if (System.nanoTime() > deadline)
          throw new AssertionError("connect to s_server failed", connect.cause());
        else Thread.sleep(50);
      }
      try {
        SslHandler ssl = channel.pipeline().get(SslHandler.class);
        ssl.handshakeFuture().sync();
        require(
            "h2".equals(ssl.applicationProtocol()), "s_server ALPN " + ssl.applicationProtocol());
        channel.writeAndFlush(Unpooled.copiedBuffer("hello-native\n", CharsetUtil.US_ASCII));
        String reply = new String(collector.done.get(10, TimeUnit.SECONDS), CharsetUtil.US_ASCII);
        require(reply.equals("evitan-olleh\n"), "s_server reply " + reply);
      } finally {
        channel.close().syncUninterruptibly();
      }
    } finally {
      process.destroy();
      process.waitFor(5, TimeUnit.SECONDS);
    }
    System.out.println("NativeSslEngine client against OpenSSL s_server passed");
  }

  static String readUntil(InputStream stream, String... needles) throws Exception {
    StringBuilder output = new StringBuilder();
    CompletableFuture<String> done = new CompletableFuture<>();
    Thread reader =
        new Thread(
            () -> {
              try {
                int next;
                while ((next = stream.read()) >= 0) {
                  output.append((char) next);
                  synchronized (output) {
                    if (Arrays.stream(needles).allMatch(output.toString()::contains)) {
                      done.complete(output.toString());
                      return;
                    }
                  }
                }
                done.completeExceptionally(new AssertionError("s_client exited:\n" + output));
              } catch (Exception error) {
                done.completeExceptionally(error);
              }
            });
    reader.setDaemon(true);
    reader.start();
    return done.get(15, TimeUnit.SECONDS);
  }

  static void require(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }
}
