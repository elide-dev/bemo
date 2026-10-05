import dev.elide.bemo.transport.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import io.netty.channel.epoll.*;
import io.netty.channel.kqueue.*;
import io.netty.channel.nio.NioIoHandler;
import io.netty.channel.socket.nio.*;
import io.netty.handler.codec.DateFormatter;
import io.netty.handler.codec.http.*;
import io.netty.handler.ssl.*;
import java.io.ByteArrayInputStream;
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
  private static long dateSecond = -1;
  private static String dateValue;

  private static String date() {
    long second = System.currentTimeMillis() / 1000;
    if (second != dateSecond) {
      dateValue = DateFormatter.format(new java.util.Date(second * 1000));
      dateSecond = second;
    }
    return dateValue;
  }

  private static final class Reply extends SimpleChannelInboundHandler<FullHttpResponse> {
    private final byte[] payload;
    private volatile CompletableFuture<Void> pending;
    private long started;
    private long latency;

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
        latency = System.nanoTime() - started;
        pending.complete(null);
      }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
      if (pending != null) pending.completeExceptionally(error);
      context.close();
    }
  }

  private static long exercise(
      List<Channel> clients, List<Reply> replies, int rounds, boolean gzip, long[] latencies)
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
        reply.started = System.nanoTime();
        clients
            .get(i)
            .writeAndFlush(request)
            .addListener(
                future -> {
                  if (!future.isSuccess()) reply.pending.completeExceptionally(future.cause());
                });
      }
      CompletableFuture.allOf(batch).get(10, TimeUnit.SECONDS);
      if (latencies != null) {
        for (int i = 0; i < clients.size(); i++)
          latencies[round * clients.size() + i] = replies.get(i).latency;
      }
    }
    return System.nanoTime() - started;
  }

  private static String rss(String field, long serverPid) throws Exception {
    if (serverPid != 0) {
      String client = rss(field, 0);
      Path status = Path.of("/proc", Long.toString(serverPid), "status");
      if (client.equals("null") || !Files.isRegularFile(status)) return "null";
      for (String line : Files.readAllLines(status)) {
        if (line.startsWith(field + ":")) {
          return Long.toString(
              Long.parseLong(client) + Long.parseLong(line.split("\\s+")[1]) * 1024);
        }
      }
      throw new IllegalStateException("Missing server RSS: " + field);
    }
    Path status = Path.of("/proc/self/status");
    if (!Files.isRegularFile(status)) return "null";
    for (String line : Files.readAllLines(status)) {
      if (line.startsWith(field + ":"))
        return Long.toString(Long.parseLong(line.split("\\s+")[1]) * 1024);
    }
    throw new IllegalStateException("Missing Linux RSS metric: " + field);
  }

  public static void main(String[] args) throws Exception {
    boolean serverOnly = args.length > 12 && args[12].equals("server");
    int externalPort = args.length > 12 && !serverOnly ? Integer.parseInt(args[12]) : 0;
    long serverPid = args.length > 13 ? Long.parseLong(args[13]) : 0;
    Path library = Path.of(args[0]);
    byte[] cert = Files.readAllBytes(Path.of(args[1]));
    byte[] key = Files.readAllBytes(Path.of(args[2]));
    boolean tls = Boolean.parseBoolean(args[3]);
    boolean gzip = Boolean.parseBoolean(args[4]);
    int size = Integer.parseInt(args[5]);
    int rounds = Integer.parseInt(args[6]);
    int warmup = Integer.parseInt(args[7]);
    String transport = args[8];
    int clientCount = Integer.parseInt(args[9]);
    String tlsProvider = args[10];
    int backend = Integer.parseInt(args[11]);
    if (clientCount < 1 || clientCount > 1024)
      throw new IllegalArgumentException("Clients must be between 1 and 1024");
    if (!tlsProvider.equals("jdk") && !tlsProvider.equals("native"))
      throw new IllegalArgumentException("Unknown TLS provider: " + tlsProvider);
    if (tlsProvider.equals("native") && !transport.equals("bemo"))
      throw new IllegalArgumentException("Native Rust TLS requires Bemo");
    if (rounds < 1 || warmup < 1 || size < 1)
      throw new IllegalArgumentException("Positive workload required");
    byte[] payload = new byte[size];
    byte[] pattern =
        "{\"message\":\"bemo transport benchmark\",\"value\":12345}\n"
            .getBytes(java.nio.charset.StandardCharsets.UTF_8);
    for (int i = 0; i < size; i++) payload[i] = pattern[i % pattern.length];
    TransportNative api = transport.equals("bemo") ? new FfmTransportNative(library) : null;
    boolean nativeTls = tls && tlsProvider.equals("native");
    NativeTlsContext serverTls =
        nativeTls ? NativeTlsContext.server(api, cert, key, "http/1.1") : null;
    NativeTlsContext clientTls = nativeTls ? NativeTlsContext.client(api, cert, "http/1.1") : null;
    SslContext serverSsl =
        tls && !nativeTls
            ? SslContextBuilder.forServer(
                    new ByteArrayInputStream(cert), new ByteArrayInputStream(key))
                .sslProvider(SslProvider.JDK)
                .protocols("TLSv1.3")
                .build()
            : null;
    SslContext clientSsl =
        tls && !nativeTls
            ? SslContextBuilder.forClient()
                .trustManager(new ByteArrayInputStream(cert))
                .sslProvider(SslProvider.JDK)
                .protocols("TLSv1.3")
                .build()
            : null;
    IoHandlerFactory factory;
    Class<? extends ServerChannel> serverClass;
    Class<? extends Channel> clientClass;
    switch (transport) {
      case "bemo" -> {
        factory = NativeIoHandler.newFactory(api, backend, 256, 64 * 1024 * 1024);
        serverClass = NativeServerSocketChannel.class;
        clientClass = NativeSocketChannel.class;
      }
      case "epoll" -> {
        Epoll.ensureAvailability();
        factory = EpollIoHandler.newFactory();
        serverClass = EpollServerSocketChannel.class;
        clientClass = EpollSocketChannel.class;
      }
      case "kqueue" -> {
        KQueue.ensureAvailability();
        factory = KQueueIoHandler.newFactory();
        serverClass = KQueueServerSocketChannel.class;
        clientClass = KQueueSocketChannel.class;
      }
      case "nio" -> {
        factory = NioIoHandler.newFactory();
        serverClass = NioServerSocketChannel.class;
        clientClass = NioSocketChannel.class;
      }
      default -> throw new IllegalArgumentException("Unknown transport: " + transport);
    }
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(externalPort != 0 || serverOnly ? 1 : 2, factory);
    List<Channel> clients = new ArrayList<>();
    List<Reply> replies = new ArrayList<>();
    Channel server = null;
    try {
      if (externalPort == 0)
        server =
            new ServerBootstrap()
                .group(group)
                .channel(serverClass)
                .childOption(ChannelOption.TCP_NODELAY, true)
                .childHandler(
                    new ChannelInitializer<Channel>() {
                      @Override
                      protected void initChannel(Channel channel) {
                        if (nativeTls) ((NativeSocketChannel) channel).tls(serverTls, null);
                        if (serverSsl != null)
                          channel.pipeline().addLast(serverSsl.newHandler(channel.alloc()));
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
      if (serverOnly) {
        System.out.printf(
            "{\"port\":%d,\"driver\":\"%s\",\"auto_fallback\":false}%n",
            ((InetSocketAddress) server.localAddress()).getPort(), transport);
        System.out.flush();
        var control = new java.io.BufferedReader(new java.io.InputStreamReader(System.in));
        String command;
        while ((command = control.readLine()) != null && command.equals("cpu")) {
          System.out.printf(
              "{\"server_cpu_ns\":%d}%n",
              ProcessHandle.current().info().totalCpuDuration().orElseThrow().toNanos());
          System.out.flush();
        }
        return;
      }
      for (int i = 0; i < clientCount; i++) {
        Reply reply = new Reply(payload);
        replies.add(reply);
        Channel client =
            new Bootstrap()
                .group(group)
                .channelFactory(
                    () -> {
                      try {
                        Channel channel = clientClass.getConstructor().newInstance();
                        return nativeTls
                            ? ((NativeSocketChannel) channel).tls(clientTls, "localhost")
                            : channel;
                      } catch (ReflectiveOperationException error) {
                        throw new IllegalStateException(error);
                      }
                    })
                .option(ChannelOption.TCP_NODELAY, true)
                .handler(
                    new ChannelInitializer<Channel>() {
                      @Override
                      protected void initChannel(Channel channel) {
                        if (clientSsl != null) {
                          SslHandler ssl = clientSsl.newHandler(channel.alloc(), "localhost", 0);
                          var parameters = ssl.engine().getSSLParameters();
                          parameters.setEndpointIdentificationAlgorithm("HTTPS");
                          ssl.engine().setSSLParameters(parameters);
                          channel.pipeline().addLast(ssl);
                        }
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
                .connect(
                    externalPort == 0
                        ? server.localAddress()
                        : new InetSocketAddress("127.0.0.1", externalPort))
                .sync()
                .channel();
        clients.add(client);
        if (nativeTls) ((NativeSocketChannel) client).handshakeFuture().sync();
        if (clientSsl != null) client.pipeline().get(SslHandler.class).handshakeFuture().sync();
      }
      exercise(clients, replies, warmup, gzip, null);
      if (externalPort != 0) {
        System.out.println("{\"phase\":\"start\"}");
        System.out.flush();
        if (System.in.read() < 0) throw new IllegalStateException("Missing measurement controller");
      }
      String before = rss("VmRSS", serverPid);
      long cpuBefore = ProcessHandle.current().info().totalCpuDuration().orElseThrow().toNanos();
      long[] latencies = new long[Math.multiplyExact(clientCount, rounds)];
      long elapsed = exercise(clients, replies, rounds, gzip, latencies);
      long cpuAfter = ProcessHandle.current().info().totalCpuDuration().orElseThrow().toNanos();
      Arrays.sort(latencies);
      long requests = (long) clientCount * rounds;
      DriverSelection selection = transport.equals("bemo") ? DriverSelection.observed() : null;
      if (transport.equals("bemo") && selection == null)
        throw new IllegalStateException("Missing native backend observation");
      String selectedDriver = selection == null ? transport : selection.driver();
      boolean autoFallback = selection != null && selection.fallback() != null;
      System.out.printf(
          Locale.ROOT,
          "{\"transport\":\"%s\",\"driver\":\"%s\",\"auto_fallback\":%s,\"tls_provider\":\"%s\",\"tls\":%s,\"gzip\":%s,\"payload_bytes\":%d,\"clients\":%d,\"requests\":%d,\"elapsed_ns\":%d,\"requests_per_second\":%.3f,\"latency_p50_ns\":%d,\"latency_p99_ns\":%d,\"process_cpu_ns\":%d,\"rss_before_bytes\":%s,\"rss_after_bytes\":%s,\"peak_rss_bytes\":%s}%n",
          transport,
          selectedDriver,
          autoFallback,
          tls ? tlsProvider : "none",
          tls,
          gzip,
          size,
          clientCount,
          requests,
          elapsed,
          requests * 1e9 / elapsed,
          latencies[(int) Math.ceil(latencies.length * 0.50) - 1],
          latencies[(int) Math.ceil(latencies.length * 0.99) - 1],
          cpuAfter - cpuBefore,
          before,
          rss("VmRSS", serverPid),
          rss("VmHWM", serverPid));
      System.out.flush();
      if (externalPort != 0 && System.in.read() < 0)
        throw new IllegalStateException("Missing measurement completion controller");
    } finally {
      for (Channel client : clients) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      if (clientTls != null) clientTls.close();
      if (serverTls != null) serverTls.close();
      if (api != null) Workload.close(api, Workload.DEFAULT);
    }
  }
}
