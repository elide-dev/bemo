import dev.elide.bemo.transport.*;
import io.netty.bootstrap.*;
import io.netty.buffer.ByteBuf;
import io.netty.channel.*;
import java.net.InetSocketAddress;
import java.nio.file.*;
import java.util.concurrent.*;

public final class NativeTransferTest {

  public static void main(String[] args) throws Exception {
    TransportNative api = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(api, Files.readAllBytes(Path.of(args[1])), Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    verify(api, cert, key, null);
  }

  public static void verify(
      TransportNative api, byte[] cert, byte[] key, RecvByteBufAllocator allocator)
      throws Exception {
    try (NativeTlsContext serverTls = NativeTlsContext.server(api, cert, key, "http/1.1");
        NativeTlsContext clientTls = NativeTlsContext.client(api, cert, "http/1.1")) {
      transfer(api, null, null, allocator);
      transfer(api, serverTls, clientTls, allocator);
    }
  }

  private static void transfer(
      TransportNative api,
      NativeTlsContext serverTls,
      NativeTlsContext clientTls,
      RecvByteBufAllocator allocator)
      throws Exception {
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(api, 0, 128, 16 * 1024 * 1024));
    Channel server = null, client = null;
    ByteBuf retained = null;
    final int size = 2 * 1024 * 1024 + 17;
    CompletableFuture<Void> complete = new CompletableFuture<>();
    try {
      server =
          new ServerBootstrap()
              .group(group)
              .channel(NativeServerSocketChannel.class)
              .childOption(ChannelOption.AUTO_READ, false)
              .childOption(ChannelOption.RCVBUF_ALLOCATOR, allocator)
              .childHandler(
                  new ChannelInitializer<NativeSocketChannel>() {
                    @Override
                    protected void initChannel(NativeSocketChannel channel) {
                      if (serverTls != null) channel.tls(serverTls, null);
                      channel
                          .pipeline()
                          .addLast(
                              new ChannelInboundHandlerAdapter() {
                                @Override
                                public void channelActive(ChannelHandlerContext context) {
                                  context.read();
                                }

                                @Override
                                public void channelRead(
                                    ChannelHandlerContext context, Object message) {
                                  context.writeAndFlush(message);
                                }

                                @Override
                                public void channelReadComplete(ChannelHandlerContext context) {
                                  context.read();
                                }

                                @Override
                                public void exceptionCaught(
                                    ChannelHandlerContext context, Throwable error) {
                                  complete.completeExceptionally(error);
                                  context.close();
                                }
                              });
                    }
                  })
              .bind(new InetSocketAddress("127.0.0.1", 0))
              .sync()
              .channel();
      client =
          new Bootstrap()
              .option(ChannelOption.RCVBUF_ALLOCATOR, allocator)
              .group(group)
              .channelFactory(
                  () -> {
                    NativeSocketChannel channel = new NativeSocketChannel();
                    if (clientTls != null) channel.tls(clientTls, "localhost");
                    return channel;
                  })
              .handler(
                  new ChannelInboundHandlerAdapter() {
                    int offset;

                    @Override
                    public void channelRead(ChannelHandlerContext context, Object message) {
                      ByteBuf bytes = (ByteBuf) message;
                      try {
                        while (bytes.isReadable()) {
                          if (offset >= size || bytes.readByte() != (byte) offset++)
                            throw new AssertionError("Payload corruption at " + (offset - 1));
                        }
                        if (offset == size) complete.complete(null);
                      } catch (Throwable error) {
                        complete.completeExceptionally(error);
                      } finally {
                        bytes.release();
                      }
                    }

                    @Override
                    public void exceptionCaught(ChannelHandlerContext context, Throwable error) {
                      complete.completeExceptionally(error);
                      context.close();
                    }
                  })
              .connect(server.localAddress())
              .sync()
              .channel();
      if (clientTls != null) ((NativeSocketChannel) client).handshakeFuture().sync();
      ByteBuf payload = client.alloc().directBuffer(size, size);
      for (int i = 0; i < size; i++) payload.writeByte(i);
      retained = payload.retainedSlice(0, 17);
      client.writeAndFlush(payload).sync();
      complete.get(15, TimeUnit.SECONDS);
      client.close().sync();
      if (retained.refCnt() != 1)
        throw new AssertionError("completed send retained native storage");
      for (int i = 0; i < 17; i++) {
        if (retained.getByte(i) != (byte) i)
          throw new AssertionError("retained send storage corrupted");
      }
      System.out.println(
          "Native multi-record transfer and manual read checks passed: "
              + (clientTls == null ? "TCP" : "TLS"));
    } finally {
      if (retained != null) retained.release();
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
