package dev.elide.bemo.examples.quickstart;

import dev.elide.bemo.transport.DriverSelection;
import dev.elide.bemo.transport.FfmTransportNative;
import dev.elide.bemo.transport.NativeIoHandler;
import dev.elide.bemo.transport.NativeServerSocketChannel;
import dev.elide.bemo.transport.Workload;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.MultiThreadIoEventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http.HttpVersion;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;

/** A release consumer: Bemo sockets with Netty HTTP/1.1 codecs on the JVM. */
public final class Application {
  public static void main(String[] args) throws Exception {
    var transport = new FfmTransportNative();
    var group =
        new MultiThreadIoEventLoopGroup(
            2, NativeIoHandler.newFactory(transport, 0, 128, 8 * 1024 * 1024));
    try {
      Channel server =
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
                              new HttpServerCodec(),
                              new HttpObjectAggregator(8192),
                              new SimpleChannelInboundHandler<FullHttpRequest>() {
                                @Override
                                protected void channelRead0(
                                    ChannelHandlerContext ctx, FullHttpRequest request) {
                                  byte[] body = "Hello, World!".getBytes(StandardCharsets.UTF_8);
                                  var response =
                                      new DefaultFullHttpResponse(
                                          HttpVersion.HTTP_1_1,
                                          HttpResponseStatus.OK,
                                          Unpooled.wrappedBuffer(body));
                                  response
                                      .headers()
                                      .set(HttpHeaderNames.CONTENT_TYPE, "text/plain");
                                  response
                                      .headers()
                                      .setInt(HttpHeaderNames.CONTENT_LENGTH, body.length);
                                  ctx.writeAndFlush(response);
                                }
                              });
                    }
                  })
              .bind(new InetSocketAddress("127.0.0.1", Integer.getInteger("server.port", 8080)))
              .sync()
              .channel();
      Runtime.getRuntime()
          .addShutdownHook(
              new Thread(
                  () -> {
                    server.close().syncUninterruptibly();
                    group.shutdownGracefully().syncUninterruptibly();
                    Workload.close(transport, Workload.DEFAULT);
                    System.out.println("Bemo stopped; workload released");
                  }));
      System.out.println("Bemo FFM: " + DriverSelection.observed());
      System.out.println(
          "GET http://127.0.0.1:"
              + ((InetSocketAddress) server.localAddress()).getPort()
              + "/plaintext");
      server.closeFuture().sync();
    } catch (Throwable error) {
      group.shutdownGracefully().syncUninterruptibly();
      Workload.close(transport, Workload.DEFAULT);
      throw error;
    }
  }
}
