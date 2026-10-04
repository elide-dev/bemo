import dev.elide.netty.v2.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import java.net.*;
import java.nio.file.Path;
import java.util.concurrent.*;

public final class NativeLifecycleTest {

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(api);
  }

  public static void verify(TransportNative api) throws Exception {
    NativeDestroyContract.verify(api);
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            1, NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024));
    try (ServerSocket listener = new ServerSocket(0, 16, InetAddress.getLoopbackAddress())) {
      ChannelFuture rejectedBind =
          new io.netty.bootstrap.ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .childHandler(new ChannelInboundHandlerAdapter())
              .bind(listener.getLocalSocketAddress())
              .await();
      if (rejectedBind.isSuccess() || !(rejectedBind.cause().getCause() instanceof BindException))
        throw new AssertionError("Bind error lost its type", rejectedBind.cause());
      rejectedBind.channel().close().sync();
      NativeSocketChannel channel =
          (NativeSocketChannel)
              new Bootstrap()
                  .group(group)
                  .channel(NativeSocketChannel.class)
                  .handler(new ChannelInboundHandlerAdapter())
                  .connect(listener.getLocalSocketAddress())
                  .sync()
                  .channel();
      try (Socket peer = listener.accept()) {
        peer.setSoTimeout(5000);
        channel.config().setKeepAlive(true);
        if (!channel.config().isKeepAlive())
          throw new AssertionError("cross-thread socket option lost");
        CompletableFuture<ChannelFuture> second = new CompletableFuture<>();
        channel
            .eventLoop()
            .execute(
                () -> {
                  channel.writeAndFlush(Unpooled.buffer(1).writeByte(1));
                  second.complete(channel.write(Unpooled.buffer(1).writeByte(2)));
                });
        ChannelFuture queued = second.get(5, TimeUnit.SECONDS);
        if (peer.getInputStream().read() != 1) throw new AssertionError("first write missing");
        channel.eventLoop().submit(() -> {}).sync();
        if (queued.isDone()) throw new AssertionError("unflushed promise completed");
        channel.flush();
        queued.sync();
        if (peer.getInputStream().read() != 2) throw new AssertionError("explicit flush missing");
        channel.shutdownInput().sync();
        channel.eventLoop().submit(() -> {}).sync();
        if (!channel.isOpen() || channel.isOutputShutdown())
          throw new AssertionError("input shutdown closed output");
        channel.writeAndFlush(Unpooled.buffer(1).writeByte(3)).sync();
        if (peer.getInputStream().read() != 3)
          throw new AssertionError("half-closed write missing");
        channel.shutdownOutput().sync();
        if (channel.writeAndFlush(Unpooled.buffer(1).writeByte(4)).await().isSuccess())
          throw new AssertionError("write after shutdown succeeded");
        if (!channel.isOpen()) throw new AssertionError("output shutdown closed channel");
        channel.close().sync();
      }
      Channel channel2 =
          new Bootstrap()
              .group(group)
              .channel(NativeSocketChannel.class)
              .handler(new ChannelInboundHandlerAdapter())
              .connect(listener.getLocalSocketAddress())
              .sync()
              .channel();
      try (Socket peer = listener.accept()) {
        channel2.deregister().sync();
        channel2.closeFuture().sync();
        if (channel2.isOpen())
          throw new AssertionError("live deregistration retained a native socket");
      }
      System.out.println("Native flush, half-close, and deregistration checks passed");
    } finally {
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
