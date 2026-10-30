package dev.elide.bemo.transport;

import io.netty.bootstrap.Bootstrap;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.IoRegistration;
import io.netty.channel.MultiThreadIoEventLoopGroup;
import io.netty.util.concurrent.EventExecutor;
import java.lang.reflect.Field;
import java.net.InetSocketAddress;
import java.nio.file.Path;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

/**
 * A single failed accept (EMFILE/ENFILE, ECONNABORTED, transport-quota exhaustion, ...) must only
 * discard the failed connection and keep the listener accepting, mirroring Netty's {@code
 * AbstractNioMessageChannel}, which catches per-accept {@code IOException} and leaves the server
 * channel open. This contract fabricates a negative accept completion and delivers it through
 * {@code NativeUnsafe.handle} on a registered listener, then verifies the listener is still live
 * and still serves a real connection.
 *
 * <p>To deliver the fabricated completion, the listener's outstanding-accept handle is set to a
 * sentinel value that the native driver never armed, so no real accept operation is left dangling
 * (a "phantom" that would otherwise claim and drop the next real connection). Auto-read is left off
 * until after the fabrication, then a fresh real accept is armed before connecting.
 */
public final class NativeAcceptResilienceTest {
  public static void main(String[] args) throws Exception {
    verify(new FfmTransportNative(Path.of(args[0])));
  }

  public static void verify(TransportNative api) throws Exception {
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(api, backendCode(), 128, 8 * 1024 * 1024));
    Channel server = null;
    Channel client = null;
    try {
      CompletableFuture<Throwable> acceptFailure = new CompletableFuture<>();
      server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .option(ChannelOption.AUTO_READ, false)
              .handler(
                  new ChannelInboundHandlerAdapter() {
                    @Override
                    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
                      acceptFailure.complete(error);
                    }
                  })
              .childHandler(
                  new ChannelInitializer<Channel>() {
                    @Override
                    protected void initChannel(Channel child) {
                      child
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
      NativeServerSocketChannel listener = (NativeServerSocketChannel) server;
      Field accept = NativeServerSocketChannel.class.getDeclaredField("accept");
      accept.setAccessible(true);
      // Sentinel handle that the driver never armed, so completed() admits the fabricated accept.
      accept.setLong(listener, 1L);
      NativeIoHandler.Completion failed = new NativeIoHandler.Completion();
      failed.kind = 2;
      failed.operation = 1L;
      failed.result = -(1000 + 24); // EMFILE via the generic OS-error mapping
      failed.value = 0;
      IoRegistration registration = listener.registration();
      ((EventExecutor) listener.eventLoop())
          .submit(
              () -> {
                ((NativeIoHandler.Handle) listener.unsafe()).handle(registration, failed);
                return null;
              })
          .sync();
      Throwable cause = acceptFailure.get(5, TimeUnit.SECONDS);
      if (cause == null)
        throw new AssertionError("expected exceptionCaught on the listener pipeline");
      if (!cause.getMessage().contains("accept") || !cause.getMessage().contains("OS error 24"))
        throw new AssertionError("unexpected accept-failure cause: " + cause);
      // The listener must survive the failed accept (the bug force-closed it).
      if (!server.isOpen()) throw new AssertionError("listener closed by a failed accept");
      if (!server.isActive())
        throw new AssertionError("listener went inactive after a failed accept");
      if (accept.getLong(listener) != 0)
        throw new AssertionError("listener still has a stale accept handle after a failed accept");
      // Arm a fresh real accept, then prove a real connection is still served end-to-end.
      ((EventExecutor) listener.eventLoop()).submit(listener::read).sync();
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
      if (reply.get(5, TimeUnit.SECONDS) != 42)
        throw new AssertionError("echo after failed accept mismatch");
      System.out.println("Native accept-resilience checks passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }

  /**
   * Selects the driver backend from ELIDE_TRANSPORT_TEST_BACKEND (0 auto, 1 polling, 2 io-uring).
   */
  static int backendCode() {
    String name = System.getenv("ELIDE_TRANSPORT_TEST_BACKEND");
    if (name == null || name.equals("auto")) return 0;
    return switch (name) {
      case "kqueue", "epoll" -> 1;
      case "io-uring" -> 2;
      case "iocp" -> 3;
      default -> throw new AssertionError("Unknown backend: " + name);
    };
  }
}
