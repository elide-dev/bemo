import dev.elide.bemo.transport.*;
import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import io.netty.channel.socket.ChannelOutputShutdownEvent;
import io.netty.channel.socket.DuplexChannel;
import java.net.InetSocketAddress;
import java.nio.file.Path;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Admission refusal from the native driver (`socketSend` == 0) is transient backpressure and must
 * keep the write queued for retry, never half-close the channel or fail the queued write.
 */
public final class NativeBackpressureTest {

  static final class RefusingTransport extends BackendTransport {
    final AtomicInteger sendRefusals = new AtomicInteger();
    private int refuseSends;

    RefusingTransport(TransportNative delegate, int refuseSends) {
      super(delegate);
      this.refuseSends = refuseSends;
    }

    @Override
    public boolean supportsInlineWrites() {
      return false;
    }

    @Override
    public boolean supportsInlineVectoredWrites() {
      return false;
    }

    @Override
    public long socketSend(
        long workload, long driver, long socket, long buffer, long offset, long length) {
      if (refuseSends > 0) {
        refuseSends--;
        sendRefusals.incrementAndGet();
        return 0;
      }
      return super.socketSend(workload, driver, socket, buffer, offset, length);
    }
  }

  public static void main(String[] args) throws Exception {
    TransportNative real = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verifyStagedRefusal(real);
    System.out.println("Native backpressure deferral checks passed");
  }

  static void verifyStagedRefusal(TransportNative real) throws Exception {
    int refused = 3;
    RefusingTransport api = new RefusingTransport(real, refused);
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(1, NativeIoHandler.newFactory(api, 0, 16, 8 * 1024 * 1024));
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
                      io.netty.buffer.ByteBuf bytes = (io.netty.buffer.ByteBuf) message;
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
                    public void userEventTriggered(ChannelHandlerContext context, Object event) {
                      if (event instanceof ChannelOutputShutdownEvent) {
                        reply.completeExceptionally(
                            new AssertionError(
                                "ChannelOutputShutdownEvent fired; admission refusal half-closed"
                                    + " the channel"));
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
      ChannelFuture write = client.writeAndFlush(Unpooled.buffer(4).writeInt(42));
      if (reply.get(10, TimeUnit.SECONDS) != 42) {
        throw new AssertionError("native echo mismatch after admission-refusal deferral");
      }
      if (!write.await(5, TimeUnit.SECONDS) || !write.isSuccess()) {
        throw new AssertionError("queued write did not succeed after deferral", write.cause());
      }
      if (!client.isActive())
        throw new AssertionError("client became inactive after admission refusal");
      if (((DuplexChannel) client).isOutputShutdown())
        throw new AssertionError("client output was half-closed");
      if (api.sendRefusals.get() < refused) {
        throw new AssertionError(
            "admission refusal not exercised as expected: " + api.sendRefusals.get());
      }
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
