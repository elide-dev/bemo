import dev.elide.bemo.transport.*;
import io.netty.bootstrap.*;
import io.netty.channel.*;
import java.net.InetSocketAddress;
import java.nio.file.*;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Verifies that a second {@code close(promise)} issued while a void-promise TLS close is in flight
 * completes the promise successfully rather than failing with {@code IllegalStateException("void
 * future")}. Without the fix, {@code NativeTlsSession.requestClose} calls {@code
 * closePromise.addListener(...)} without checking {@code isVoid()}, and {@code
 * VoidChannelPromise.addListener} throws unconditionally.
 */
public final class NativeTlsDoubleCloseTest {

  public static void main(String[] args) throws Exception {
    TransportNative delegate = new BackendTransport(new FfmTransportNative(Path.of(args[0])));
    verify(delegate, Files.readAllBytes(Path.of(args[1])), Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative delegate, byte[] cert, byte[] key) throws Exception {
    AtomicBoolean closeInitiated = new AtomicBoolean();
    TransportNative api =
        new BackendTransport(delegate) {
          @Override
          public boolean supportsReceiveResults() {
            return false;
          }

          @Override
          public int tlsStep(
              long session, int action, long plaintext, long offset, long length, long output) {
            if (action == 3) closeInitiated.set(true);
            return super.tlsStep(session, action, plaintext, offset, length, output);
          }

          @Override
          public long socketSend(
              long workload, long driver, long socket, long buffer, long offset, long length) {
            if (closeInitiated.get()) return Long.MAX_VALUE;
            return super.socketSend(workload, driver, socket, buffer, offset, length);
          }
        };
    EventLoopGroup group =
        new MultiThreadIoEventLoopGroup(
            1, NativeIoHandler.newFactory(api, 0, 128, 8 * 1024 * 1024));
    Channel server = null;
    NativeSocketChannel client = null;
    java.util.concurrent.CompletableFuture<NativeSocketChannel> accepted =
        new java.util.concurrent.CompletableFuture<>();
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
                      accepted.complete(channel);
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
      client.handshakeFuture().sync();
      NativeSocketChannel peer = accepted.get(5, TimeUnit.SECONDS);
      peer.handshakeFuture().sync();
      ChannelPromise secondPromise = client.newPromise();
      NativeSocketChannel c = client;
      group
          .next()
          .execute(
              () -> {
                c.unsafe().close(c.voidPromise());
                c.unsafe().close(secondPromise);
              });
      if (!secondPromise.await(10, TimeUnit.SECONDS))
        throw new AssertionError("second close promise was never completed (hung)");
      if (!secondPromise.isSuccess())
        throw new AssertionError("second close promise failed with: " + secondPromise.cause());
      if (secondPromise.cause() != null)
        throw new AssertionError("second close promise has unexpected cause");
      if (!client.closeFuture().await(10, TimeUnit.SECONDS))
        throw new AssertionError("closeFuture did not complete (channel hung)");
      if (!client.closeFuture().isSuccess())
        throw new AssertionError("closeFuture did not succeed");
      System.out.println("Native TLS double-close (void then real promise) passed");
    } finally {
      closeInitiated.set(false);
      if (client != null) client.close().syncUninterruptibly();
      if (server != null) server.close().syncUninterruptibly();
      group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
    }
  }
}
