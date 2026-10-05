import dev.elide.bemo.transport.*;
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

  private static void connectReadOrdering(TransportNative api) throws Exception {
    CountDownLatch peerWritten = new CountDownLatch(1);
    CompletableFuture<Void> received = new CompletableFuture<>();
    TransportNative delayedConnect =
        new BackendTransport(api) {
          private boolean notified;

          @Override
          public int driverPoll(long driver, long timeout, long batch, int maximum) {
            int count = super.driverPoll(driver, timeout, batch, maximum);
            if (count > 0 && !notified) {
              notified = true;
              try {
                if (!peerWritten.await(5, TimeUnit.SECONDS))
                  throw new AssertionError("peer did not queue greeting before connect completion");
              } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new AssertionError(error);
              }
            }
            return count;
          }
        };
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            1, NativeIoHandler.newFactory(delayedConnect, 0, 128, 1024 * 1024));
    Channel channel = null;
    try (ServerSocket listener = new ServerSocket(0, 16, InetAddress.getLoopbackAddress())) {
      CompletableFuture<Void> peer =
          CompletableFuture.runAsync(
              () -> {
                try (Socket socket = listener.accept()) {
                  socket.getOutputStream().write(42);
                  peerWritten.countDown();
                  received.get(5, TimeUnit.SECONDS);
                } catch (Exception error) {
                  throw new CompletionException(error);
                }
              });
      channel =
          new Bootstrap()
              .group(group)
              .channel(NativeSocketChannel.class)
              .option(ChannelOption.AUTO_READ, false)
              .handler(
                  new ChannelInboundHandlerAdapter() {
                    private boolean active;

                    @Override
                    public void channelActive(ChannelHandlerContext context) {
                      active = true;
                      context.fireChannelActive();
                    }

                    @Override
                    public void channelRead(ChannelHandlerContext context, Object message) {
                      io.netty.buffer.ByteBuf bytes = (io.netty.buffer.ByteBuf) message;
                      try {
                        if (!active) throw new AssertionError("read preceded channelActive");
                        if (bytes.readByte() != 42) throw new AssertionError("greeting mismatch");
                        received.complete(null);
                      } catch (Throwable error) {
                        received.completeExceptionally(error);
                      } finally {
                        bytes.release();
                      }
                    }

                    @Override
                    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
                      received.completeExceptionally(error);
                      context.close();
                    }
                  })
              .register()
              .sync()
              .channel();
      ChannelPromise connect = channel.newPromise();
      connect.addListener(
          done -> {
            if (done.isSuccess()) connect.channel().read();
            else received.completeExceptionally(done.cause());
          });
      channel.connect(listener.getLocalSocketAddress(), connect).sync();
      received.get(5, TimeUnit.SECONDS);
      peer.get(5, TimeUnit.SECONDS);
    } finally {
      if (channel != null) channel.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }

  public static void verify(TransportNative api) throws Exception {
    NativeDestroyContract.verify(api);
    connectReadOrdering(api);
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            1, NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024));
    try (ServerSocket listener = new ServerSocket(0, 16, InetAddress.getLoopbackAddress())) {
      CountDownLatch tasks = new CountDownLatch(8 * 256);
      Thread[] producers = new Thread[8];
      for (int i = 0; i < producers.length; i++) {
        producers[i] =
            new Thread(
                () -> {
                  for (int j = 0; j < 256; j++) group.next().execute(tasks::countDown);
                });
        producers[i].start();
      }
      for (Thread producer : producers) producer.join(5000);
      if (!tasks.await(5, TimeUnit.SECONDS))
        throw new AssertionError("coalesced wake lost queued tasks");
      for (int i = 0; i < 32; i++) {
        CompletableFuture<Void> nested = new CompletableFuture<>();
        group.next().execute(() -> group.next().execute(() -> nested.complete(null)));
        nested.get(5, TimeUnit.SECONDS);
      }
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
