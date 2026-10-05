import dev.elide.bemo.transport.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.SocketAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

/** Callback closure and deregistration must not orphan TLS sessions. */
public final class NativeReentrantCloseTest {

  public static void main(String[] args) throws Exception {
    verify(
        new BackendTransport(new FfmTransportNative(Path.of(args[0]))),
        Files.readAllBytes(Path.of(args[1])),
        Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative delegate, byte[] cert, byte[] key) throws Exception {
    AtomicInteger sessions = new AtomicInteger();
    AtomicInteger created = new AtomicInteger();
    TransportNative api =
        new BackendTransport(delegate) {
          @Override
          public long tlsNew(long workload, long context, long owner, long name) {
            long session = super.tlsNew(workload, context, owner, name);
            if (session != 0) {
              created.incrementAndGet();
              sessions.incrementAndGet();
            }
            return session;
          }

          @Override
          public int tlsRelease(long session) {
            int result = super.tlsRelease(session);
            if (result == 0) sessions.decrementAndGet();
            return result;
          }
        };
    try (NativeTlsContext clientTls = NativeTlsContext.client(api, cert, "h2");
        NativeTlsContext serverTls = NativeTlsContext.server(api, cert, key, "h2")) {
      EventLoopGroup group =
          new MultiThreadIoEventLoopGroup(
              1, NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024));
      Channel server = null;
      Channel client = null;
      try {
        AtomicInteger active = new AtomicInteger();
        AtomicInteger connected = new AtomicInteger();
        try (ServerSocket listener = new ServerSocket(0, 16, InetAddress.getLoopbackAddress())) {
          client =
              new Bootstrap()
                  .group(group)
                  .channelFactory(() -> new NativeSocketChannel().tls(clientTls, "localhost"))
                  .handler(
                      new ChannelDuplexHandler() {
                        @Override
                        public void connect(
                            ChannelHandlerContext ctx,
                            SocketAddress remote,
                            SocketAddress local,
                            ChannelPromise promise) {
                          promise.addListener(
                              done -> {
                                connected.incrementAndGet();
                                ctx.writeAndFlush(Unpooled.buffer(1).writeByte(7));
                                ctx.read();
                                ctx.close();
                              });
                          ctx.connect(remote, local, promise);
                        }

                        @Override
                        public void channelActive(ChannelHandlerContext ctx) {
                          active.incrementAndGet();
                          ctx.fireChannelActive();
                        }
                      })
                  .connect(listener.getLocalSocketAddress())
                  .sync()
                  .channel();
          try (Socket peer = listener.accept()) {
            client.closeFuture().sync();
            client.eventLoop().submit(() -> {}).sync();
            if (created.get() != 0 || active.get() != 0 || connected.get() != 1)
              throw new AssertionError("Connect listener close still started TLS or fired active");
            var closedHandshake = ((NativeSocketChannel) client).handshakeFuture();
            if (!closedHandshake.await(1, TimeUnit.SECONDS)
                || !(closedHandshake.cause() instanceof java.nio.channels.ClosedChannelException))
              throw new AssertionError("Closed channel left a pending or successful handshake");
            if (created.get() != 0)
              throw new AssertionError("Closed handshake created a TLS session");
            peer.setSoTimeout(3000);
            if (peer.getInputStream().read() != -1)
              throw new AssertionError("Closed TLS connect sent bytes");
          }
        }
        CompletableFuture<Void> received = new CompletableFuture<>();
        server =
            new ServerBootstrap()
                .group(group)
                .channel(NativeServerSocketChannel.class)
                .childHandler(
                    new ChannelInitializer<Channel>() {
                      @Override
                      protected void initChannel(Channel channel) {
                        ((NativeSocketChannel) channel).tls(serverTls, null);
                        channel
                            .pipeline()
                            .addLast(
                                new ChannelInboundHandlerAdapter() {
                                  @Override
                                  public void channelRead(
                                      ChannelHandlerContext ctx, Object message) {
                                    ByteBuf bytes = (ByteBuf) message;
                                    try {
                                      if (bytes.readableBytes() != 1 || bytes.readByte() != 7) {
                                        received.completeExceptionally(
                                            new AssertionError("Connect listener write mismatch"));
                                      } else received.complete(null);
                                    } finally {
                                      bytes.release();
                                    }
                                  }

                                  @Override
                                  public void exceptionCaught(
                                      ChannelHandlerContext ctx, Throwable cause) {
                                    received.completeExceptionally(cause);
                                    ctx.close();
                                  }
                                });
                      }
                    })
                .bind(new InetSocketAddress("127.0.0.1", 0))
                .sync()
                .channel();
        client =
            new Bootstrap()
                .group(group)
                .channelFactory(() -> new NativeSocketChannel().tls(clientTls, "localhost"))
                .handler(
                    new ChannelDuplexHandler() {
                      @Override
                      public void connect(
                          ChannelHandlerContext ctx,
                          SocketAddress remote,
                          SocketAddress local,
                          ChannelPromise promise) {
                        promise.addListener(
                            done -> {
                              ctx.writeAndFlush(Unpooled.buffer(1).writeByte(7));
                              ctx.read();
                            });
                        ctx.connect(remote, local, promise);
                      }
                    })
                .connect(server.localAddress())
                .sync()
                .channel();
        ((NativeSocketChannel) client).handshakeFuture().sync();
        received.get(5, TimeUnit.SECONDS);
        client.deregister().sync();
        if (client.isOpen() || !client.closeFuture().isDone())
          throw new AssertionError(
              "TLS deregistration deferred close after detaching completion delivery");
      } finally {
        if (client != null) client.close();
        if (server != null) server.close();
        group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      }
      shutdownLiveTls(api, clientTls, serverTls);
      if (sessions.get() != 0) throw new AssertionError("Orphaned TLS sessions: " + sessions.get());
    }
    System.out.println("Native callback closure and TLS deregistration checks passed");
  }

  private static void shutdownLiveTls(
      TransportNative api, NativeTlsContext clientTls, NativeTlsContext serverTls)
      throws Exception {
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            1, NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024));
    AtomicInteger inactive = new AtomicInteger();
    AtomicInteger unregistered = new AtomicInteger();
    AtomicBoolean lateCallback = new AtomicBoolean();
    CompletableFuture<Channel> accepted = new CompletableFuture<>();
    try {
      Channel server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .childHandler(
                  new ChannelInitializer<Channel>() {
                    @Override
                    protected void initChannel(Channel channel) {
                      ((NativeSocketChannel) channel).tls(serverTls, null);
                      channel.pipeline().addLast(lifecycle(inactive, unregistered, lateCallback));
                      accepted.complete(channel);
                    }
                  })
              .bind(new InetSocketAddress("127.0.0.1", 0))
              .sync()
              .channel();
      Channel client =
          new Bootstrap()
              .group(group)
              .channelFactory(() -> new NativeSocketChannel().tls(clientTls, "localhost"))
              .handler(lifecycle(inactive, unregistered, lateCallback))
              .connect(server.localAddress())
              .sync()
              .channel();
      Channel peer = accepted.get(5, TimeUnit.SECONDS);
      ((NativeSocketChannel) client).handshakeFuture().sync();
      ((NativeSocketChannel) peer).handshakeFuture().sync();
      // Flip shutdown while draining tasks, after the loop's initial shutdown check.
      group.next().execute(() -> group.shutdownGracefully(0, 5, TimeUnit.SECONDS));
      if (!group.terminationFuture().await(10, TimeUnit.SECONDS))
        throw new AssertionError("Live TLS shutdown did not terminate");
      if (client.isOpen()
          || peer.isOpen()
          || server.isOpen()
          || client.isRegistered()
          || peer.isRegistered()
          || server.isRegistered())
        throw new AssertionError("Shutdown retained a live or registered TLS channel");
      if (inactive.get() != 2 || unregistered.get() != 2 || lateCallback.get())
        throw new AssertionError(
            "Shutdown lost or delayed TLS lifecycle callbacks: inactive="
                + inactive.get()
                + ", unregistered="
                + unregistered.get());
    } finally {
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }

  private static ChannelHandler lifecycle(
      AtomicInteger inactive, AtomicInteger unregistered, AtomicBoolean lateCallback) {
    return new ChannelInboundHandlerAdapter() {
      @Override
      public void channelInactive(ChannelHandlerContext ctx) {
        if (ctx.executor().isTerminated()) lateCallback.set(true);
        inactive.incrementAndGet();
        ctx.fireChannelInactive();
      }

      @Override
      public void channelUnregistered(ChannelHandlerContext ctx) {
        if (ctx.executor().isTerminated()) lateCallback.set(true);
        unregistered.incrementAndGet();
        ctx.fireChannelUnregistered();
      }

      @Override
      public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        ctx.close();
      }
    };
  }
}
