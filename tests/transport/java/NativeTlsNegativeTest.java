import dev.elide.bemo.transport.*;
import io.netty.bootstrap.*;
import io.netty.channel.*;
import java.net.InetSocketAddress;
import java.nio.file.*;
import java.util.concurrent.TimeUnit;

public final class NativeTlsNegativeTest {

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(api, Files.readAllBytes(Path.of(args[1])), Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024));
    Channel server = null;
    NativeSocketChannel client = null;
    try (NativeTlsContext serverTls = NativeTlsContext.server(api, cert, key, "h2");
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
                      channel
                          .pipeline()
                          .addLast(
                              new ChannelInboundHandlerAdapter() {
                                @Override
                                public void exceptionCaught(
                                    ChannelHandlerContext context, Throwable error) {
                                  context.close();
                                }
                              });
                    }
                  })
              .bind(new InetSocketAddress("127.0.0.1", 0))
              .sync()
              .channel();
      client =
          (NativeSocketChannel)
              new Bootstrap()
                  .group(group)
                  .channelFactory(() -> new NativeSocketChannel().tls(clientTls, "wrong.example"))
                  .handler(
                      new ChannelInboundHandlerAdapter() {
                        @Override
                        public void exceptionCaught(
                            ChannelHandlerContext context, Throwable error) {
                          context.close();
                        }
                      })
                  .connect(server.localAddress())
                  .sync()
                  .channel();
      var handshake = client.handshakeFuture();
      if (!handshake.await(5, TimeUnit.SECONDS) || handshake.isSuccess())
        throw new AssertionError("Wrong hostname accepted or hung");
      if (!client.closeFuture().await(5, TimeUnit.SECONDS))
        throw new AssertionError("Failed handshake left socket open");
      System.out.println("Native TLS hostname rejection passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
