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
import io.netty.channel.MultiThreadIoEventLoopGroup;
import io.netty.util.concurrent.EventExecutor;
import java.lang.reflect.InvocationTargetException;
import java.net.InetSocketAddress;
import java.nio.file.Path;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Exercises the {@code beginNativeRead()} admission-rejection branch: when the driver's transient
 * operation limit is hit, {@code socketAccept} returns {@code 0}, and the fixed listener must
 * return without throwing or force-closing (the bug threw {@code NativeTransportException("Native
 * accept admission failed")} which propagated to {@code NativeUnsafe.handle} / {@code
 * AbstractUnsafe.beginRead} and force-closed the listener).
 *
 * <p>A dynamic proxy wraps the real {@link TransportNative} and returns {@code 0} for the first
 * {@code socketAccept} call only, then delegates to the real binding. With {@code AUTO_READ=false},
 * the first {@code listener.read()} hits the admission-rejection branch; the listener must stay
 * open. A second {@code listener.read()} arms a real accept; a real connection then echoes.
 */
public final class NativeAcceptAdmissionTest {
  public static void main(String[] args) throws Exception {
    verify(new FfmTransportNative(Path.of(args[0])));
  }

  public static void verify(TransportNative real) throws Exception {
    AtomicInteger acceptCalls = new AtomicInteger();
    TransportNative api =
        (TransportNative)
            java.lang.reflect.Proxy.newProxyInstance(
                TransportNative.class.getClassLoader(),
                new Class<?>[] {TransportNative.class},
                (proxy, method, args) -> {
                  if (method.getName().equals("socketAccept")
                      && acceptCalls.incrementAndGet() == 1) {
                    return 0L; // admission rejected: driver transient operation limit hit
                  }
                  try {
                    return method.invoke(real, args);
                  } catch (InvocationTargetException e) {
                    throw e.getCause();
                  }
                });
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2,
            NativeIoHandler.newFactory(
                api, NativeAcceptResilienceTest.backendCode(), 128, 8 * 1024 * 1024));
    Channel server = null;
    Channel client = null;
    try {
      server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .option(ChannelOption.AUTO_READ, false)
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
      EventExecutor loop = (EventExecutor) listener.eventLoop();
      // First read: beginNativeRead() -> socketAccept returns 0 -> returns without arming/throwing.
      CompletableFuture<Void> firstRead = new CompletableFuture<>();
      loop.execute(
          () -> {
            listener.read();
            firstRead.complete(null);
          });
      firstRead.get(5, TimeUnit.SECONDS);
      if (acceptCalls.get() < 1)
        throw new AssertionError("admission-rejection socketAccept was not invoked");
      if (!server.isOpen())
        throw new AssertionError("listener closed by admission rejection (beginNativeRead threw)");
      if (!server.isActive())
        throw new AssertionError("listener went inactive after admission rejection");
      // Second read: socketAccept now delegates; a real accept arms.
      CompletableFuture<Void> secondRead = new CompletableFuture<>();
      loop.execute(
          () -> {
            listener.read();
            secondRead.complete(null);
          });
      secondRead.get(5, TimeUnit.SECONDS);
      // A real connection must be accepted and echoed end-to-end.
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
        throw new AssertionError("echo after admission rejection mismatch");
      System.out.println("Native accept-admission checks passed");
    } finally {
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
