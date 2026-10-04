import dev.elide.dokar.transport.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import io.netty.handler.codec.http.*;
import java.net.InetSocketAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/** Fixed-work loopback benchmark: native transport, Netty HTTP/1 and optional gzip/TLS. */
@SuppressWarnings("deprecation")
public final class TransportBenchmark {
  private static final int CLIENTS = 4;

  private static final class Reply extends SimpleChannelInboundHandler<FullHttpResponse> {
    private final byte[] payload;
    private CompletableFuture<Void> pending;

    Reply(byte[] payload) {
      this.payload = payload;
    }

    @Override
    protected void channelRead0(ChannelHandlerContext context, FullHttpResponse response) {
      byte[] bytes = new byte[response.content().readableBytes()];
      response.content().readBytes(bytes);
      if (!response.status().equals(HttpResponseStatus.OK) || !Arrays.equals(bytes, payload)) {
        pending.completeExceptionally(new AssertionError("HTTP payload mismatch"));
      } else {
        pending.complete(null);
      }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
      if (pending != null) pending.completeExceptionally(error);
      context.close();
    }
  }

  private static long exercise(List<Channel> clients, List<Reply> replies, int rounds, boolean gzip)
      throws Exception {
    long started = System.nanoTime();
    for (int round = 0; round < rounds; round++) {
      CompletableFuture<?>[] batch = new CompletableFuture<?>[clients.size()];
      for (int i = 0; i < clients.size(); i++) {
        Reply reply = replies.get(i);
        reply.pending = new CompletableFuture<>();
        batch[i] = reply.pending;
        FullHttpRequest request =
            new DefaultFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/payload");
        request.headers().set(HttpHeaderNames.HOST, "localhost");
        request.headers().set(HttpHeaderNames.ACCEPT_ENCODING, gzip ? "gzip" : "identity");
        clients
            .get(i)
            .writeAndFlush(request)
            .addListener(
                future -> {
                  if (!future.isSuccess()) reply.pending.completeExceptionally(future.cause());
                });
      }
      CompletableFuture.allOf(batch).get(10, TimeUnit.SECONDS);
    }
    return System.nanoTime() - started;
  }

  private static String rss(String field) throws Exception {
    Path status = Path.of("/proc/self/status");
    if (!Files.isRegularFile(status)) return "null";
    for (String line : Files.readAllLines(status)) {
      if (line.startsWith(field + ":"))
        return Long.toString(Long.parseLong(line.split("\\s+")[1]) * 1024);
    }
    throw new IllegalStateException("Missing Linux RSS metric: " + field);
  }

  public static void main(String[] args) throws Exception {
    Path library = Path.of(args[0]);
    byte[] cert = Files.readAllBytes(Path.of(args[1]));
    byte[] key = Files.readAllBytes(Path.of(args[2]));
    boolean tls = Boolean.parseBoolean(args[3]);
    boolean gzip = Boolean.parseBoolean(args[4]);
    int size = Integer.parseInt(args[5]);
    int rounds = Integer.parseInt(args[6]);
    int warmup = Integer.parseInt(args[7]);
    if (rounds < 1 || warmup < 1 || size < 1)
      throw new IllegalArgumentException("Positive workload required");
    byte[] payload = new byte[size];
    byte[] pattern =
        "{\"message\":\"dokar transport benchmark\",\"value\":12345}\n"
            .getBytes(java.nio.charset.StandardCharsets.UTF_8);
    for (int i = 0; i < size; i++) payload[i] = pattern[i % pattern.length];
    TransportNative api = new FfmTransportNative(library);
    NativeTlsContext serverTls = tls ? NativeTlsContext.server(api, cert, key, "http/1.1") : null;
    NativeTlsContext clientTls = tls ? NativeTlsContext.client(api, cert, "http/1.1") : null;
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(api, 0, 256, 64 * 1024 * 1024));
    List<Channel> clients = new ArrayList<>();
    List<Reply> replies = new ArrayList<>();
    Channel server = null;
    try {
      server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .childOption(ChannelOption.TCP_NODELAY, true)
              .childHandler(
                  new ChannelInitializer<Channel>() {
                    @Override
                    protected void initChannel(Channel channel) {
                      if (tls) ((NativeSocketChannel) channel).tls(serverTls, null);
                      channel
                          .pipeline()
                          .addLast(new HttpServerCodec(), new HttpObjectAggregator(1024 * 1024));
                      if (gzip) channel.pipeline().addLast(new HttpContentCompressor());
                      channel
                          .pipeline()
                          .addLast(
                              new SimpleChannelInboundHandler<FullHttpRequest>() {
                                @Override
                                protected void channelRead0(
                                    ChannelHandlerContext ctx, FullHttpRequest request) {
                                  FullHttpResponse response =
                                      new DefaultFullHttpResponse(
                                          HttpVersion.HTTP_1_1,
                                          HttpResponseStatus.OK,
                                          Unpooled.wrappedBuffer(payload));
                                  response
                                      .headers()
                                      .set(HttpHeaderNames.CONTENT_TYPE, "application/json");
                                  response
                                      .headers()
                                      .setInt(HttpHeaderNames.CONTENT_LENGTH, payload.length);
                                  ctx.writeAndFlush(response);
                                }
                              });
                    }
                  })
              .bind(new InetSocketAddress("127.0.0.1", 0))
              .sync()
              .channel();
      for (int i = 0; i < CLIENTS; i++) {
        Reply reply = new Reply(payload);
        replies.add(reply);
        Channel client =
            new Bootstrap()
                .group(group)
                .channelFactory(
                    () -> {
                      NativeSocketChannel channel = new NativeSocketChannel();
                      return tls ? channel.tls(clientTls, "localhost") : channel;
                    })
                .option(ChannelOption.TCP_NODELAY, true)
                .handler(
                    new ChannelInitializer<Channel>() {
                      @Override
                      protected void initChannel(Channel channel) {
                        channel
                            .pipeline()
                            .addLast(
                                new HttpClientCodec(),
                                new ChannelInboundHandlerAdapter() {
                                  @Override
                                  public void channelRead(
                                      ChannelHandlerContext ctx, Object message) {
                                    if (message instanceof HttpResponse response) {
                                      boolean encoded =
                                          "gzip"
                                              .equals(
                                                  response
                                                      .headers()
                                                      .get(HttpHeaderNames.CONTENT_ENCODING));
                                      if (encoded != gzip) {
                                        io.netty.util.ReferenceCountUtil.release(message);
                                        reply.pending.completeExceptionally(
                                            new AssertionError("Compression negotiation mismatch"));
                                        ctx.close();
                                        return;
                                      }
                                    }
                                    ctx.fireChannelRead(message);
                                  }
                                },
                                new HttpContentDecompressor(),
                                new HttpObjectAggregator(1024 * 1024),
                                reply);
                      }
                    })
                .connect(server.localAddress())
                .sync()
                .channel();
        clients.add(client);
        if (tls) ((NativeSocketChannel) client).handshakeFuture().sync();
      }
      exercise(clients, replies, warmup, gzip);
      String before = rss("VmRSS");
      long elapsed = exercise(clients, replies, rounds, gzip);
      long requests = (long) CLIENTS * rounds;
      System.out.printf(
          Locale.ROOT,
          "{\"tls\":%s,\"gzip\":%s,\"payload_bytes\":%d,\"clients\":%d,\"requests\":%d,\"elapsed_ns\":%d,\"requests_per_second\":%.3f,\"rss_before_bytes\":%s,\"rss_after_bytes\":%s,\"peak_rss_bytes\":%s}%n",
          tls,
          gzip,
          size,
          CLIENTS,
          requests,
          elapsed,
          requests * 1e9 / elapsed,
          before,
          rss("VmRSS"),
          rss("VmHWM"));
    } finally {
      for (Channel client : clients) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      if (clientTls != null) clientTls.close();
      if (serverTls != null) serverTls.close();
    }
  }
}
