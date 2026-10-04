import dev.elide.netty.v2.*;
import dev.elide.netty.v2.tls.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufAllocator;
import io.netty.buffer.Unpooled;
import io.netty.buffer.UnpooledByteBufAllocator;
import io.netty.channel.*;
import io.netty.channel.nio.NioIoHandler;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import io.netty.handler.codec.http.*;
import io.netty.handler.codec.http2.*;
import io.netty.handler.ssl.*;
import io.netty.util.CharsetUtil;
import io.netty.util.ReferenceCountUtil;
import java.io.ByteArrayOutputStream;
import java.net.InetSocketAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.cert.X509Certificate;
import java.util.Arrays;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;
import javax.net.ssl.SSLHandshakeException;

/** Netty SslHandler contracts over NativeSslEngine, on v2 sockets and on NIO with heap buffers. */
public final class NativeSslEngineTest {

  record Stack(
      String name,
      EventLoopGroup group,
      Class<? extends ServerChannel> server,
      Class<? extends Channel> client,
      ByteBufAllocator allocator) {}

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(api, Files.readAllBytes(Path.of(args[1])), Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    workloadClosure(api, cert);
    EventLoopGroup v2 =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(api, 0, 128, 32 * 1024 * 1024));
    EventLoopGroup nio = new MultiThreadIoEventLoopGroup(2, NioIoHandler.newFactory());
    NativeSslContext server =
        NativeSslContextBuilder.forServer(api, cert, key)
            .applicationProtocols(
                ApplicationProtocolNames.HTTP_2, ApplicationProtocolNames.HTTP_1_1)
            .build();
    NativeSslContext client =
        NativeSslContextBuilder.forClient(api)
            .trustAnchors(cert)
            .applicationProtocols(
                ApplicationProtocolNames.HTTP_2, ApplicationProtocolNames.HTTP_1_1)
            .build();
    NativeSslContext http1Client =
        NativeSslContextBuilder.forClient(api)
            .trustAnchors(cert)
            .applicationProtocols(ApplicationProtocolNames.HTTP_1_1)
            .build();
    NativeSslContext plainClient =
        NativeSslContextBuilder.forClient(api).trustAnchors(cert).build();
    NativeSslContext insecureClient =
        NativeSslContextBuilder.forClient(api)
            .insecureSkipVerify(true)
            .applicationProtocols(
                ApplicationProtocolNames.HTTP_2, ApplicationProtocolNames.HTTP_1_1)
            .build();
    try {
      for (Stack stack :
          new Stack[] {
            new Stack("v2", v2, NativeServerSocketChannel.class, NativeSocketChannel.class, null),
            new Stack(
                "nio-heap",
                nio,
                NioServerSocketChannel.class,
                NioSocketChannel.class,
                new UnpooledByteBufAllocator(false)),
          }) {
        echoAndSession(stack, server, client);
        echoAndSession(stack, server, insecureClient);
        closeNotify(stack, server, client);
        wrongName(stack, server, client);
        http1(stack, server, http1Client, ApplicationProtocolNames.HTTP_1_1);
        http1(stack, server, plainClient, null);
        http2(stack, server, client);
        System.out.println("SslHandler contracts over NativeSslEngine passed: " + stack.name());
      }
      derContext(api, cert, key, v2);
    } finally {
      for (NativeSslContext context :
          new NativeSslContext[] {
            server, client, http1Client, plainClient, insecureClient,
          }) {
        // Engines release their context when SslHandler leaves the pipeline, after close completes.
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        while (context.refCnt() > 1 && System.nanoTime() < deadline) Thread.sleep(10);
        if (context.refCnt() != 1 || !context.release())
          throw new AssertionError("Context references leaked");
      }
      v2.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      nio.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }

  static void workloadClosure(TransportNative api, byte[] cert) throws Exception {
    long owner = api.ownerNew(16 * 1024 * 1024);
    long sibling = api.ownerNew(16 * 1024 * 1024);
    NativeSslContext first =
        NativeSslContextBuilder.forClient(api).workload(owner).trustAnchors(cert).build();
    NativeSslContext second =
        NativeSslContextBuilder.forClient(api).workload(sibling).trustAnchors(cert).build();
    var active = first.newEngine(null, "localhost", 443);
    var other = second.newEngine(null, "localhost", 443);
    try {
      active.beginHandshake();
      require(api.workloadClose(owner) == 0, "close TLS workload");
      try {
        active.wrap(java.nio.ByteBuffer.allocate(0), java.nio.ByteBuffer.allocate(32768));
        throw new AssertionError("Closed TLS workload admitted wrap");
      } catch (javax.net.ssl.SSLException expected) {
        require(
            expected.getMessage().contains("workload is closed"), "workload failure diagnostic");
      }
      other.beginHandshake();
      require(
          other
                  .wrap(java.nio.ByteBuffer.allocate(0), java.nio.ByteBuffer.allocate(32768))
                  .bytesProduced()
              > 0,
          "sibling TLS workload remains open");
    } finally {
      ReferenceCountUtil.release(active);
      ReferenceCountUtil.release(other);
      first.release();
      second.release();
      require(api.ownerRelease(owner) == 0, "release TLS workload");
      require(api.ownerRelease(sibling) == 0, "release sibling TLS workload");
    }
    System.out.println("Native TLS engine workload isolation checks passed");
  }

  static Channel listen(Stack stack, Consumer<Channel> initializer) throws Exception {
    ServerBootstrap bootstrap = new ServerBootstrap().group(stack.group()).channel(stack.server());
    if (stack.allocator() != null)
      bootstrap.childOption(ChannelOption.ALLOCATOR, stack.allocator());
    return bootstrap
        .childHandler(
            new ChannelInitializer<Channel>() {
              @Override
              protected void initChannel(Channel channel) {
                initializer.accept(channel);
              }
            })
        .bind(new InetSocketAddress("127.0.0.1", 0))
        .sync()
        .channel();
  }

  static Channel connect(Stack stack, Channel server, Consumer<Channel> initializer)
      throws Exception {
    Bootstrap bootstrap = new Bootstrap().group(stack.group()).channel(stack.client());
    if (stack.allocator() != null) bootstrap.option(ChannelOption.ALLOCATOR, stack.allocator());
    return bootstrap
        .handler(
            new ChannelInitializer<Channel>() {
              @Override
              protected void initChannel(Channel channel) {
                initializer.accept(channel);
              }
            })
        .connect(server.localAddress())
        .sync()
        .channel();
  }

  static SslHandler clientHandler(SslContext context, Channel channel, String host) {
    return context.newHandler(channel.alloc(), host, 443);
  }

  static final class Collector extends ChannelInboundHandlerAdapter {

    final ByteArrayOutputStream received = new ByteArrayOutputStream();
    final CompletableFuture<byte[]> done = new CompletableFuture<>();
    final int expected;

    Collector(int expected) {
      this.expected = expected;
    }

    @Override
    public void channelRead(ChannelHandlerContext context, Object message) {
      ByteBuf bytes = (ByteBuf) message;
      try {
        byte[] chunk = new byte[bytes.readableBytes()];
        bytes.readBytes(chunk);
        received.writeBytes(chunk);
        if (received.size() >= expected) done.complete(received.toByteArray());
      } finally {
        bytes.release();
      }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
      done.completeExceptionally(error);
      context.close();
    }
  }

  /** Expected handshake failures end at this handler instead of the pipeline tail. */
  static final class Quiet extends ChannelInboundHandlerAdapter {

    @Override
    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
      context.close();
    }
  }

  static final class Echo extends ChannelInboundHandlerAdapter {

    @Override
    public void channelRead(ChannelHandlerContext context, Object message) {
      context.write(message);
    }

    @Override
    public void channelReadComplete(ChannelHandlerContext context) {
      context.flush();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
      context.close();
    }
  }

  static void echoAndSession(
      Stack stack, NativeSslContext serverContext, NativeSslContext clientContext)
      throws Exception {
    CompletableFuture<String> serverProtocol = new CompletableFuture<>();
    Channel server =
        listen(
            stack,
            channel -> {
              channel.pipeline().addLast(serverContext.newHandler(channel.alloc()));
              channel
                  .pipeline()
                  .addLast(
                      new ApplicationProtocolNegotiationHandler(ApplicationProtocolNames.HTTP_1_1) {
                        @Override
                        protected void configurePipeline(
                            ChannelHandlerContext context, String protocol) {
                          serverProtocol.complete(protocol);
                          context.pipeline().addLast(new Echo());
                        }
                      });
            });
    byte[] payload = new byte[1024 * 1024 + 1000 * 10];
    for (int i = 0; i < payload.length; i++) payload[i] = (byte) (i * 31 + 7);
    Collector collector = new Collector(payload.length);
    CompletableFuture<String> clientProtocol = new CompletableFuture<>();
    Channel channel =
        connect(
            stack,
            server,
            ch -> {
              ch.pipeline().addLast(clientHandler(clientContext, ch, "localhost"));
              ch.pipeline()
                  .addLast(
                      new ApplicationProtocolNegotiationHandler(ApplicationProtocolNames.HTTP_1_1) {
                        @Override
                        protected void configurePipeline(
                            ChannelHandlerContext context, String protocol) {
                          clientProtocol.complete(protocol);
                          context.pipeline().addLast(collector);
                        }
                      });
            });
    try {
      SslHandler ssl = channel.pipeline().get(SslHandler.class);
      ssl.handshakeFuture().sync();
      require("h2".equals(clientProtocol.get(10, TimeUnit.SECONDS)), "client ALPN");
      require("h2".equals(serverProtocol.get(10, TimeUnit.SECONDS)), "server ALPN");
      require("h2".equals(ssl.applicationProtocol()), "SslHandler.applicationProtocol");
      var session = ssl.engine().getSession();
      require("TLSv1.3".equals(session.getProtocol()), "protocol " + session.getProtocol());
      require(
          session.getCipherSuite().startsWith("TLS_AES_")
              || session.getCipherSuite().startsWith("TLS_CHACHA20"),
          "suite " + session.getCipherSuite());
      require(session.getId().length == 32, "session id");
      var peer = (X509Certificate) session.getPeerCertificates()[0];
      require(peer.getSubjectX500Principal().getName().contains("127.0.0.1"), "peer certificate");
      require(session.getPacketBufferSize() >= 16384 + 5, "packet size");
      // One large write crosses many records; a burst of small writes exercises coalescing.
      channel.write(Unpooled.wrappedBuffer(payload, 0, 1024 * 1024));
      for (int i = 0; i < 1000; i++)
        channel.write(Unpooled.wrappedBuffer(payload, 1024 * 1024 + i * 10, 10));
      channel.flush();
      require(Arrays.equals(collector.done.get(20, TimeUnit.SECONDS), payload), "echo mismatch");
    } finally {
      channel.close().syncUninterruptibly();
      server.close().syncUninterruptibly();
    }
  }

  static void closeNotify(
      Stack stack, NativeSslContext serverContext, NativeSslContext clientContext)
      throws Exception {
    CompletableFuture<SslHandler> serverClosed = new CompletableFuture<>();
    Channel server =
        listen(
            stack,
            channel -> {
              channel.pipeline().addLast(serverContext.newHandler(channel.alloc()));
              channel
                  .pipeline()
                  .addLast(
                      new ChannelInboundHandlerAdapter() {
                        @Override
                        public void userEventTriggered(
                            ChannelHandlerContext context, Object event) {
                          if (event instanceof SslCloseCompletionEvent closed
                              && closed.isSuccess()) {
                            SslHandler ssl = context.pipeline().get(SslHandler.class);
                            // TLS 1.3 half-close: output stays open, as with JSSE.
                            context
                                .writeAndFlush(Unpooled.copiedBuffer("bye", CharsetUtil.US_ASCII))
                                .addListener(done -> serverClosed.complete(ssl));
                          }
                          context.fireUserEventTriggered(event);
                        }
                      });
            });
    Collector collector = new Collector(3);
    Channel channel =
        connect(
            stack,
            server,
            ch -> {
              ch.pipeline().addLast(clientHandler(clientContext, ch, "localhost"));
              ch.pipeline().addLast(collector);
            });
    try {
      SslHandler ssl = channel.pipeline().get(SslHandler.class);
      ssl.handshakeFuture().sync();
      ssl.closeOutbound().sync();
      require(ssl.engine().isOutboundDone(), "client outbound done");
      SslHandler peer = serverClosed.get(10, TimeUnit.SECONDS);
      require(peer.engine().isInboundDone(), "server inbound done");
      require(!peer.engine().isOutboundDone(), "server output stays open");
      require(
          "bye".equals(new String(collector.done.get(10, TimeUnit.SECONDS), CharsetUtil.US_ASCII)),
          "half-close write");
    } finally {
      channel.close().syncUninterruptibly();
      server.close().syncUninterruptibly();
    }
  }

  static void wrongName(Stack stack, NativeSslContext serverContext, NativeSslContext clientContext)
      throws Exception {
    CompletableFuture<Throwable> serverFailure = new CompletableFuture<>();
    Channel server =
        listen(
            stack,
            channel -> {
              SslHandler ssl = serverContext.newHandler(channel.alloc());
              ssl.handshakeFuture().addListener(done -> serverFailure.complete(done.cause()));
              channel.pipeline().addLast(ssl, new Quiet());
            });
    CompletableFuture<Throwable> clientFailure = new CompletableFuture<>();
    Channel channel =
        connect(
            stack,
            server,
            ch -> {
              SslHandler ssl = clientHandler(clientContext, ch, "example.com");
              ssl.handshakeFuture().addListener(done -> clientFailure.complete(done.cause()));
              ch.pipeline().addLast(ssl, new Quiet());
            });
    try {
      Throwable failure = clientFailure.get(10, TimeUnit.SECONDS);
      require(failure instanceof SSLHandshakeException, "handshake exception " + failure);
      require(failure.getMessage().contains("certificate"), "failure text " + failure.getMessage());
      require(serverFailure.get(10, TimeUnit.SECONDS) != null, "server saw the alert");
    } finally {
      channel.close().syncUninterruptibly();
      server.close().syncUninterruptibly();
    }
  }

  static void http1(
      Stack stack, NativeSslContext serverContext, NativeSslContext clientContext, String offered)
      throws Exception {
    CompletableFuture<String> serverProtocol = new CompletableFuture<>();
    Channel server =
        listen(
            stack,
            channel -> {
              channel.pipeline().addLast(serverContext.newHandler(channel.alloc()));
              channel
                  .pipeline()
                  .addLast(
                      new ApplicationProtocolNegotiationHandler(ApplicationProtocolNames.HTTP_1_1) {
                        @Override
                        protected void configurePipeline(
                            ChannelHandlerContext context, String protocol) {
                          serverProtocol.complete(protocol);
                          context
                              .pipeline()
                              .addLast(
                                  new HttpServerCodec(),
                                  new HttpObjectAggregator(65536),
                                  new SimpleChannelInboundHandler<FullHttpRequest>() {
                                    @Override
                                    protected void channelRead0(
                                        ChannelHandlerContext ctx, FullHttpRequest request) {
                                      var response =
                                          new DefaultFullHttpResponse(
                                              HttpVersion.HTTP_1_1,
                                              HttpResponseStatus.OK,
                                              Unpooled.copiedBuffer(
                                                  "h1 " + request.uri(), CharsetUtil.US_ASCII));
                                      HttpUtil.setContentLength(
                                          response, response.content().readableBytes());
                                      ctx.writeAndFlush(response);
                                    }
                                  });
                        }
                      });
            });
    CompletableFuture<String> body = new CompletableFuture<>();
    Channel channel =
        connect(
            stack,
            server,
            ch -> {
              ch.pipeline().addLast(clientHandler(clientContext, ch, "localhost"));
              ch.pipeline()
                  .addLast(
                      new HttpClientCodec(),
                      new HttpObjectAggregator(65536),
                      new SimpleChannelInboundHandler<FullHttpResponse>() {
                        @Override
                        protected void channelRead0(
                            ChannelHandlerContext ctx, FullHttpResponse response) {
                          body.complete(response.content().toString(CharsetUtil.US_ASCII));
                        }

                        @Override
                        public void exceptionCaught(ChannelHandlerContext ctx, Throwable error) {
                          body.completeExceptionally(error);
                        }
                      });
            });
    try {
      SslHandler ssl = channel.pipeline().get(SslHandler.class);
      ssl.handshakeFuture().sync();
      require(
          java.util.Objects.equals(ssl.applicationProtocol(), offered),
          "client ALPN " + ssl.applicationProtocol());
      channel.writeAndFlush(
          new DefaultFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/hello"));
      require("h1 /hello".equals(body.get(10, TimeUnit.SECONDS)), "HTTP/1.1 body");
      require(
          ApplicationProtocolNames.HTTP_1_1.equals(serverProtocol.get(10, TimeUnit.SECONDS)),
          "server fallback");
    } finally {
      channel.close().syncUninterruptibly();
      server.close().syncUninterruptibly();
    }
  }

  static void http2(Stack stack, NativeSslContext serverContext, NativeSslContext clientContext)
      throws Exception {
    Channel server =
        listen(
            stack,
            channel -> {
              channel.pipeline().addLast(serverContext.newHandler(channel.alloc()));
              channel
                  .pipeline()
                  .addLast(
                      new ApplicationProtocolNegotiationHandler(ApplicationProtocolNames.HTTP_1_1) {
                        @Override
                        protected void configurePipeline(
                            ChannelHandlerContext context, String protocol) {
                          if (!ApplicationProtocolNames.HTTP_2.equals(protocol)) {
                            context.close();
                            return;
                          }
                          context
                              .pipeline()
                              .addLast(
                                  Http2FrameCodecBuilder.forServer().build(),
                                  new Http2MultiplexHandler(
                                      new ChannelInitializer<Channel>() {
                                        @Override
                                        protected void initChannel(Channel stream) {
                                          stream
                                              .pipeline()
                                              .addLast(
                                                  new ChannelInboundHandlerAdapter() {
                                                    @Override
                                                    public void channelRead(
                                                        ChannelHandlerContext ctx, Object message) {
                                                      if (message
                                                              instanceof Http2HeadersFrame headers
                                                          && headers.isEndStream()) {
                                                        ctx.write(
                                                            new DefaultHttp2HeadersFrame(
                                                                new DefaultHttp2Headers()
                                                                    .status("200")));
                                                        ctx.writeAndFlush(
                                                            new DefaultHttp2DataFrame(
                                                                Unpooled.copiedBuffer(
                                                                    "h2 "
                                                                        + headers.headers().path(),
                                                                    CharsetUtil.US_ASCII),
                                                                true));
                                                      }
                                                      ReferenceCountUtil.release(message);
                                                    }
                                                  });
                                        }
                                      }));
                        }
                      });
            });
    CompletableFuture<Channel> ready = new CompletableFuture<>();
    Channel channel =
        connect(
            stack,
            server,
            ch -> {
              ch.pipeline().addLast(clientHandler(clientContext, ch, "localhost"));
              ch.pipeline()
                  .addLast(
                      new ApplicationProtocolNegotiationHandler(ApplicationProtocolNames.HTTP_1_1) {
                        @Override
                        protected void configurePipeline(
                            ChannelHandlerContext context, String protocol) {
                          if (!ApplicationProtocolNames.HTTP_2.equals(protocol)) {
                            ready.completeExceptionally(
                                new AssertionError("negotiated " + protocol));
                            return;
                          }
                          context
                              .pipeline()
                              .addLast(
                                  Http2FrameCodecBuilder.forClient().build(),
                                  new Http2MultiplexHandler(new ChannelInboundHandlerAdapter()));
                          ready.complete(context.channel());
                        }
                      });
            });
    try {
      Channel parent = ready.get(10, TimeUnit.SECONDS);
      CompletableFuture<String> body = new CompletableFuture<>();
      ByteArrayOutputStream data = new ByteArrayOutputStream();
      Http2StreamChannel stream =
          new Http2StreamChannelBootstrap(parent)
              .handler(
                  new ChannelInboundHandlerAdapter() {
                    @Override
                    public void channelRead(ChannelHandlerContext ctx, Object message) {
                      if (message instanceof Http2DataFrame frame) {
                        byte[] chunk = new byte[frame.content().readableBytes()];
                        frame.content().readBytes(chunk);
                        data.writeBytes(chunk);
                        if (frame.isEndStream()) body.complete(data.toString(CharsetUtil.US_ASCII));
                      }
                      ReferenceCountUtil.release(message);
                    }
                  })
              .open()
              .sync()
              .getNow();
      stream.writeAndFlush(
          new DefaultHttp2HeadersFrame(
              new DefaultHttp2Headers()
                  .method("GET")
                  .path("/stream")
                  .scheme("https")
                  .authority("localhost"),
              true));
      require("h2 /stream".equals(body.get(10, TimeUnit.SECONDS)), "HTTP/2 body");
    } finally {
      channel.close().syncUninterruptibly();
      server.close().syncUninterruptibly();
    }
  }

  /** Certificates and keys arrive as JDK objects and cross the ABI as DER. */
  static void derContext(TransportNative api, byte[] cert, byte[] key, EventLoopGroup group)
      throws Exception {
    var factory = java.security.cert.CertificateFactory.getInstance("X.509");
    X509Certificate certificate =
        (X509Certificate) factory.generateCertificate(new java.io.ByteArrayInputStream(cert));
    String pem = new String(key, CharsetUtil.US_ASCII).replaceAll("-----[A-Z ]+-----|\\s", "");
    var privateKey =
        java.security.KeyFactory.getInstance("RSA")
            .generatePrivate(
                new java.security.spec.PKCS8EncodedKeySpec(
                    java.util.Base64.getDecoder().decode(pem)));
    NativeSslContext server =
        NativeSslContextBuilder.forServer(api, privateKey, certificate).build();
    NativeSslContext client =
        NativeSslContextBuilder.forClient(api).trustAnchors(java.util.List.of(certificate)).build();
    try {
      var engine = server.newEngine(null);
      require(engine.getSession().getLocalCertificates() != null, "local certificates");
      ReferenceCountUtil.release(engine);
      echoAndSessionWithoutAlpn(
          new Stack(
              "v2-der", group, NativeServerSocketChannel.class, NativeSocketChannel.class, null),
          server,
          client);
      System.out.println("DER-configured NativeSslContext checks passed");
    } finally {
      server.release();
      client.release();
    }
  }

  static void echoAndSessionWithoutAlpn(
      Stack stack, NativeSslContext serverContext, NativeSslContext clientContext)
      throws Exception {
    Channel server =
        listen(
            stack,
            channel ->
                channel.pipeline().addLast(serverContext.newHandler(channel.alloc()), new Echo()));
    Collector collector = new Collector(5);
    Channel channel =
        connect(
            stack,
            server,
            ch -> ch.pipeline().addLast(clientHandler(clientContext, ch, "127.0.0.1"), collector));
    try {
      SslHandler ssl = channel.pipeline().get(SslHandler.class);
      ssl.handshakeFuture().sync();
      require(ssl.applicationProtocol() == null, "no ALPN");
      channel.writeAndFlush(Unpooled.copiedBuffer("hello", CharsetUtil.US_ASCII));
      require(
          "hello"
              .equals(new String(collector.done.get(10, TimeUnit.SECONDS), CharsetUtil.US_ASCII)),
          "DER echo");
    } finally {
      channel.close().syncUninterruptibly();
      server.close().syncUninterruptibly();
    }
  }

  static void require(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }
}
