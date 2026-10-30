import dev.elide.bemo.transport.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import java.net.InetSocketAddress;
import java.nio.file.Path;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/**
 * Exercises transport-owned TLS against non-default workloads: a client context bound to the
 * channel's workload (via the {@code client(api, workload, ...)} overload) completes a full
 * handshake and echo over both a profile group and a {@link NativeIoHandler#forWorkload} group,
 * while a context pinned to the DEFAULT workload reports a precise workload-mismatch error instead
 * of the misleading "peer identity" diagnostic when it cannot serve such a channel.
 */
public final class NativeTlsWorkloadTest {

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(
        api,
        java.nio.file.Files.readAllBytes(Path.of(args[1])),
        java.nio.file.Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    profileGroup(api, cert, key);
    explicitWorkload(api, cert, key);
    mismatchReportsWorkloadHandles(api, cert, key);
    System.out.println("Native transport-owned TLS workload binding checks passed");
  }

  /** A profile group and a context bound to that profile complete a handshake and a 4-byte echo. */
  private static void profileGroup(TransportNative api, byte[] cert, byte[] key) throws Exception {
    long sandboxed = Workload.id(api, Workload.Profile.SANDBOXED);
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2,
            NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024, Workload.Profile.SANDBOXED));
    Channel server = null;
    Channel client = null;
    try (NativeTlsContext serverTls = NativeTlsContext.server(api, sandboxed, cert, key, "h2");
        NativeTlsContext clientTls = NativeTlsContext.client(api, sandboxed, cert, "h2")) {
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
                                    ChannelHandlerContext context, Object message) {
                                  context.writeAndFlush(message);
                                }
                              });
                    }
                  })
              .bind(new InetSocketAddress("127.0.0.1", 0))
              .sync()
              .channel();
      CompletableFuture<Integer> reply = new CompletableFuture<>();
      client =
          new Bootstrap()
              .group(group)
              .channelFactory(() -> new NativeSocketChannel().tls(clientTls, "localhost"))
              .handler(
                  new ChannelInboundHandlerAdapter() {
                    private int received;
                    private int value;

                    @Override
                    public void channelRead(ChannelHandlerContext context, Object message) {
                      ByteBuf bytes = (ByteBuf) message;
                      try {
                        while (bytes.isReadable()) {
                          if (received == 4) throw new AssertionError("excess echo bytes");
                          value = (value << 8) | bytes.readUnsignedByte();
                          if (++received == 4) reply.complete(value);
                        }
                      } finally {
                        bytes.release();
                      }
                    }

                    @Override
                    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
                      reply.completeExceptionally(error);
                    }
                  })
              .connect(server.localAddress())
              .sync()
              .channel();
      ((NativeSocketChannel) client).handshakeFuture().sync();
      if (!"h2".equals(((NativeSocketChannel) client).applicationProtocol()))
        throw new AssertionError("ALPN mismatch on profile workload");
      client.writeAndFlush(Unpooled.buffer(4).writeInt(7)).sync();
      if (reply.get(5, TimeUnit.SECONDS) != 7) throw new AssertionError("echo mismatch");
      System.out.println("  profile workload group client TLS passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }

  /** An explicit shared workload via forWorkload completes a handshake. */
  private static void explicitWorkload(TransportNative api, byte[] cert, byte[] key)
      throws Exception {
    long workload = api.ownerNew(8L * 1024 * 1024);
    if (workload == 0) throw new AssertionError("explicit workload allocation failed");
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(2, NativeIoHandler.forWorkload(api, 0, 128, workload));
    Channel server = null;
    Channel client = null;
    try (NativeTlsContext serverTls = NativeTlsContext.server(api, workload, cert, key, "h2");
        NativeTlsContext clientTls = NativeTlsContext.client(api, workload, cert, "h2")) {
      server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .childHandler(
                  new ChannelInitializer<NativeSocketChannel>() {
                    @Override
                    protected void initChannel(NativeSocketChannel channel) {
                      channel.tls(serverTls, null);
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
                  new ChannelInboundHandlerAdapter() {
                    @Override
                    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
                      context.close();
                    }
                  })
              .connect(server.localAddress())
              .sync()
              .channel();
      ((NativeSocketChannel) client).handshakeFuture().sync();
      if (!"h2".equals(((NativeSocketChannel) client).applicationProtocol()))
        throw new AssertionError("ALPN mismatch on explicit workload");
      System.out.println("  forWorkload client TLS passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      // forWorkload leaves the owner lifetime with the caller.
      api.ownerRelease(workload);
    }
  }

  /**
   * A client context pinned to the DEFAULT workload cannot serve a SANDBOXED channel: session
   * creation rejects with a workload-mismatch message naming both handles, the handshake fails, and
   * the channel closes — not a "peer identity" error.
   */
  private static void mismatchReportsWorkloadHandles(TransportNative api, byte[] cert, byte[] key)
      throws Exception {
    long sandboxed = Workload.id(api, Workload.Profile.SANDBOXED);
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2,
            NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024, Workload.Profile.SANDBOXED));
    Channel server = null;
    Channel client = null;
    // server context correctly bound to the SANDBOXED workload; client context pinned to DEFAULT.
    try (NativeTlsContext serverTls = NativeTlsContext.server(api, sandboxed, cert, key, "h2");
        NativeTlsContext clientTls = NativeTlsContext.client(api, cert, "h2")) {
      server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .childHandler(
                  new ChannelInitializer<NativeSocketChannel>() {
                    @Override
                    protected void initChannel(NativeSocketChannel channel) {
                      channel.tls(serverTls, null);
                    }
                  })
              .bind(new InetSocketAddress("127.0.0.1", 0))
              .sync()
              .channel();
      CompletableFuture<Throwable> captured = new CompletableFuture<>();
      client =
          new Bootstrap()
              .group(group)
              .channelFactory(() -> new NativeSocketChannel().tls(clientTls, "localhost"))
              .handler(
                  new ChannelInboundHandlerAdapter() {
                    @Override
                    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
                      captured.complete(error);
                      context.close();
                    }
                  })
              .connect(server.localAddress())
              .sync()
              .channel();
      NativeSocketChannel c = (NativeSocketChannel) client;
      var handshake = c.handshakeFuture();
      if (!handshake.await(5, TimeUnit.SECONDS) || handshake.isSuccess())
        throw new AssertionError("Mismatched-workload client handshake should fail");
      Throwable error = captured.get(5, TimeUnit.SECONDS);
      String message = error.getMessage();
      if (message == null || !message.contains("workload") || message.contains("peer identity"))
        throw new AssertionError("Expected a workload-mismatch diagnostic, got: " + message);
      // The diagnostic must name the DEFAULT workload and the channel's SANDBOXED workload.
      if (message.indexOf(Long.toString(Workload.id(api))) < 0
          || message.indexOf(Long.toString(sandboxed)) < 0)
        throw new AssertionError("Mismatch diagnostic must name both workloads: " + message);
      if (!client.closeFuture().await(5, TimeUnit.SECONDS))
        throw new AssertionError("Mismatched-workload channel did not close");
      System.out.println("  workload-mismatch diagnostic passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
