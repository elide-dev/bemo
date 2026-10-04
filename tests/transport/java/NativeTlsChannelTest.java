import dev.elide.dokar.transport.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import java.net.InetSocketAddress;
import java.nio.file.Path;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

public final class NativeTlsChannelTest {

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(
        api,
        java.nio.file.Files.readAllBytes(Path.of(args[1])),
        java.nio.file.Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    NativeTlsContext serverTls = NativeTlsContext.server(api, cert, key, "h2");
    NativeTlsContext clientTls = NativeTlsContext.client(api, cert, "h2");
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
        throw new AssertionError("ALPN mismatch");
      client.writeAndFlush(Unpooled.buffer(4).writeInt(42)).sync();
      if (reply.get(5, TimeUnit.SECONDS) != 42) throw new AssertionError("native echo mismatch");
      System.out.println("Native transport-owned TLS checks passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
      clientTls.close();
      serverTls.close();
    }
  }
}
