import dev.elide.netty.v2.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import java.net.InetSocketAddress;
import java.nio.file.Path;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

public final class NativeChannelTest {

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(api);
  }

  public static void verify(TransportNative api) throws Exception {
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024));
    Channel server = null;
    Channel client = null;
    try {
      server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .childHandler(
                  new ChannelInitializer<Channel>() {
                    @Override
                    protected void initChannel(Channel channel) {
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
              .channel(NativeSocketChannel.class)
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
      client.writeAndFlush(Unpooled.buffer(4).writeInt(42)).sync();
      if (reply.get(5, TimeUnit.SECONDS) != 42) throw new AssertionError("native echo mismatch");
      System.out.println("Native Netty channel checks passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
    verifyUnix(api);
  }

  private static void verifyUnix(TransportNative api) throws Exception {
    if (io.netty.util.internal.PlatformDependent.isWindows()) return;
    Path directory = java.nio.file.Files.createTempDirectory("v2u");
    Path socket = directory.resolve("s");
    EventLoopGroup loops =
        new MultiThreadIoEventLoopGroup(1, NativeIoHandler.newFactory(api, 0, 32, 1024 * 1024));
    Channel client = null;
    CompletableFuture<Void> peerDone = new CompletableFuture<>();
    try (var listener =
        java.nio.channels.ServerSocketChannel.open(java.net.StandardProtocolFamily.UNIX)) {
      var address = java.net.UnixDomainSocketAddress.of(socket);
      listener.bind(address);
      Thread peer =
          Thread.ofPlatform()
              .daemon()
              .start(
                  () -> {
                    try (var connection = listener.accept()) {
                      var bytes = java.nio.ByteBuffer.allocate(1);
                      while (bytes.hasRemaining())
                        if (connection.read(bytes) < 0) throw new java.io.EOFException();
                      bytes.flip();
                      while (bytes.hasRemaining()) connection.write(bytes);
                      peerDone.complete(null);
                    } catch (Throwable error) {
                      peerDone.completeExceptionally(error);
                    }
                  });
      CompletableFuture<Integer> reply = new CompletableFuture<>();
      client =
          new Bootstrap()
              .group(loops)
              .channelFactory(NativeDomainSocketChannel::new)
              .handler(
                  new SimpleChannelInboundHandler<ByteBuf>() {
                    @Override
                    protected void channelRead0(ChannelHandlerContext ctx, ByteBuf bytes) {
                      reply.complete((int) bytes.readUnsignedByte());
                    }

                    @Override
                    public void exceptionCaught(ChannelHandlerContext ctx, Throwable error) {
                      reply.completeExceptionally(error);
                    }
                  })
              .connect(address)
              .sync()
              .channel();
      if (!address.equals(client.remoteAddress()))
        throw new AssertionError("Unix peer address changed");
      if (!(client.localAddress() instanceof java.net.UnixDomainSocketAddress))
        throw new AssertionError("Unix local address is not a Unix endpoint");
      client.writeAndFlush(Unpooled.buffer(1).writeByte(42)).sync();
      if (reply.get(5, TimeUnit.SECONDS) != 42) throw new AssertionError("Unix echo mismatch");
      peerDone.get(5, TimeUnit.SECONDS);
      peer.join(5000);
      if (peer.isAlive()) throw new AssertionError("Unix peer did not retire");
      System.out.println("Native Unix byte-stream channel checks passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      loops.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      java.nio.file.Files.deleteIfExists(socket);
      java.nio.file.Files.deleteIfExists(directory);
    }
  }
}
