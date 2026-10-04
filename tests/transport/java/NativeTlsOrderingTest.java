import dev.elide.dokar.transport.*;
import io.netty.bootstrap.*;
import io.netty.buffer.*;
import io.netty.channel.*;
import java.net.InetSocketAddress;
import java.nio.file.*;
import java.util.concurrent.*;

public final class NativeTlsOrderingTest {

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
    CompletableFuture<Void> request = new CompletableFuture<>(),
        greeting = new CompletableFuture<>();
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
                          .handshakeFuture()
                          .addListener(
                              done -> {
                                if (done.isSuccess())
                                  channel.writeAndFlush(Unpooled.buffer(1).writeByte(7));
                              });
                      channel
                          .pipeline()
                          .addLast(
                              new ChannelInboundHandlerAdapter() {
                                @Override
                                public void channelRead(
                                    ChannelHandlerContext context, Object message) {
                                  ByteBuf bytes = (ByteBuf) message;
                                  try {
                                    if (bytes.readByte() != 8)
                                      request.completeExceptionally(
                                          new AssertionError("request mismatch"));
                                    else request.complete(null);
                                  } finally {
                                    bytes.release();
                                  }
                                }

                                @Override
                                public void exceptionCaught(
                                    ChannelHandlerContext context, Throwable error) {
                                  request.completeExceptionally(error);
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
                  .channelFactory(() -> new NativeSocketChannel().tls(clientTls, "localhost"))
                  .option(ChannelOption.AUTO_READ, false)
                  .handler(
                      new ChannelInboundHandlerAdapter() {
                        @Override
                        public void channelRead(ChannelHandlerContext context, Object message) {
                          ByteBuf bytes = (ByteBuf) message;
                          try {
                            NativeSocketChannel channel = (NativeSocketChannel) context.channel();
                            if (!channel.handshakeFuture().isSuccess()
                                || !"h2".equals(channel.applicationProtocol()))
                              throw new AssertionError("plaintext preceded handshake/ALPN");
                            if (bytes.readByte() != 7)
                              throw new AssertionError("greeting mismatch");
                            greeting.complete(null);
                          } catch (Throwable error) {
                            greeting.completeExceptionally(error);
                          } finally {
                            bytes.release();
                          }
                        }
                      })
                  .connect(server.localAddress())
                  .sync()
                  .channel();
      if (!client.handshakeFuture().await(5, TimeUnit.SECONDS)
          || !client.handshakeFuture().isSuccess())
        throw new AssertionError("manual reads blocked handshake");
      if (!client.writeAndFlush(Unpooled.buffer(1).writeByte(8)).await(3, TimeUnit.SECONDS))
        throw new AssertionError("paused plaintext blocked outbound write");
      request.get(3, TimeUnit.SECONDS);
      client.eventLoop().submit(() -> {}).sync();
      if (greeting.isDone()) throw new AssertionError("AUTO_READ=false delivered plaintext");
      client.read();
      greeting.get(3, TimeUnit.SECONDS);
      long started = System.nanoTime();
      client.close().sync();
      if (System.nanoTime() - started > TimeUnit.SECONDS.toNanos(2))
        throw new AssertionError("TLS close required timeout");
      System.out.println(
          "TLS handshake ordering, independent write progress, and close checks passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
