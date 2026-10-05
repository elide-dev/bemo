import dev.elide.bemo.transport.*;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.*;
import io.netty.channel.nio.NioIoHandler;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.util.Arrays;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** The allocator and socket backend must be independently selectable. */
public final class NativeAllocatorChannelTest {

  public static void verify(TransportNative api) {
    for (boolean v2 : new boolean[] {false, true}) {
      for (boolean nativeAllocation : new boolean[] {false, true}) {
        try (NativeByteBufAllocator nativeAllocator =
            NativeByteBufAllocator.owned(api, 1024 * 1024)) {
          ByteBufAllocator allocator =
              nativeAllocation ? nativeAllocator : ByteBufAllocator.DEFAULT;
          EventLoopGroup group =
              new MultiThreadIoEventLoopGroup(
                  1,
                  v2
                      ? NativeIoHandler.newFactory(api, 0, 128, 1024 * 1024)
                      : NioIoHandler.newFactory());
          Channel server = null;
          AtomicReference<Throwable> failure = new AtomicReference<>();
          try {
            server =
                new ServerBootstrap()
                    .group(group)
                    .channelFactory(
                        v2 ? NativeServerSocketChannel::new : NioServerSocketChannel::new)
                    .childOption(ChannelOption.ALLOCATOR, allocator)
                    .childOption(
                        ChannelOption.RCVBUF_ALLOCATOR, new FixedRecvByteBufAllocator(1024))
                    .childHandler(
                        new ChannelInitializer<Channel>() {
                          @Override
                          protected void initChannel(Channel channel) {
                            if (channel.alloc() != allocator)
                              throw new AssertionError("allocator override lost");
                            channel
                                .pipeline()
                                .addLast(
                                    new ChannelInboundHandlerAdapter() {
                                      @Override
                                      public void channelRead(
                                          ChannelHandlerContext ctx, Object message) {
                                        ByteBuf input = (ByteBuf) message;
                                        ByteBuf output =
                                            ctx.alloc().directBuffer(input.readableBytes());
                                        try {
                                          output.writeBytes(input);
                                          ctx.writeAndFlush(output);
                                        } finally {
                                          input.release();
                                        }
                                      }

                                      @Override
                                      public void exceptionCaught(
                                          ChannelHandlerContext ctx, Throwable error) {
                                        failure.set(error);
                                        ctx.close();
                                      }
                                    });
                          }
                        })
                    .bind("127.0.0.1", 0)
                    .sync()
                    .channel();
            try (Socket client = new Socket()) {
              client.setSoTimeout(5000);
              client.connect((InetSocketAddress) server.localAddress(), 5000);
              byte[] payload = new byte[4096];
              for (int i = 0; i < payload.length; i++) payload[i] = (byte) (i * 31 + 7);
              client.getOutputStream().write(payload);
              if (!Arrays.equals(payload, client.getInputStream().readNBytes(payload.length)))
                throw new AssertionError("Allocator/backend round trip corrupted payload");
            }
            if (failure.get() != null) throw new AssertionError(failure.get());
          } finally {
            if (server != null) server.close().syncUninterruptibly();
            group.shutdownGracefully(0, 2, TimeUnit.SECONDS).syncUninterruptibly();
          }
        } catch (Exception error) {
          throw new AssertionError(error);
        }
      }
    }
  }
}
