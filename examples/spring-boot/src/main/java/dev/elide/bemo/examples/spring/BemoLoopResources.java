package dev.elide.bemo.examples.spring;

import dev.elide.bemo.examples.BemoRuntime;
import dev.elide.bemo.examples.BenchmarkPayload;
import dev.elide.bemo.examples.NettyBaseline;
import dev.elide.bemo.transport.NativeIoHandler;
import dev.elide.bemo.transport.NativeServerSocketChannel;
import dev.elide.bemo.transport.NativeSocketChannel;
import io.netty.channel.Channel;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.MultiThreadIoEventLoopGroup;
import io.netty.channel.socket.ServerSocketChannel;
import io.netty.channel.socket.SocketChannel;
import java.time.Duration;
import java.util.concurrent.TimeUnit;
import reactor.core.publisher.Mono;
import reactor.netty.resources.LoopResources;

/** Reactor Netty's transport hook selects Bemo or the required native Netty baseline. */
final class BemoLoopResources implements LoopResources {
  private final int threads;
  private volatile EventLoopGroup group;

  BemoLoopResources(int threads) {
    if (threads < 1) throw new IllegalArgumentException("bemo.threads must be positive");
    this.threads = threads;
  }

  private synchronized EventLoopGroup group() {
    if (group == null)
      group =
          new MultiThreadIoEventLoopGroup(
              threads,
              BenchmarkPayload.bemoEnabled()
                  ? NativeIoHandler.newFactory(BemoRuntime.create(), 0, 256, 64 * 1024 * 1024)
                  : NettyBaseline.factory());
    return group;
  }

  @Override
  public EventLoopGroup onServer(boolean useNative) {
    return group();
  }

  @Override
  public <C extends Channel> C onChannel(Class<C> type, EventLoopGroup loops) {
    if (!BenchmarkPayload.bemoEnabled()) {
      Channel channel = NettyBaseline.channel(type);
      NettyBaseline.verifyChannel(channel);
      return type.cast(channel);
    }
    if (type == ServerSocketChannel.class) return type.cast(new NativeServerSocketChannel());
    if (type == SocketChannel.class) return type.cast(new NativeSocketChannel());
    throw new UnsupportedOperationException("Bemo example supports TCP only: " + type);
  }

  @Override
  public <C extends Channel> Class<? extends C> onChannelClass(
      Class<C> type, EventLoopGroup loops) {
    if (!BenchmarkPayload.bemoEnabled()) {
      if (type == ServerSocketChannel.class) return NettyBaseline.serverClass().asSubclass(type);
      if (type == SocketChannel.class) return NettyBaseline.socketClass().asSubclass(type);
    }
    if (type == ServerSocketChannel.class) return NativeServerSocketChannel.class.asSubclass(type);
    if (type == SocketChannel.class) return NativeSocketChannel.class.asSubclass(type);
    throw new UnsupportedOperationException("Bemo example supports TCP only: " + type);
  }

  @Override
  public Mono<Void> disposeLater(Duration quietPeriod, Duration timeout) {
    if (group == null) return Mono.empty();
    return Mono.create(
        sink ->
            group
                .shutdownGracefully(
                    quietPeriod.toMillis(), timeout.toMillis(), TimeUnit.MILLISECONDS)
                .addListener(
                    result -> {
                      if (result.isSuccess()) sink.success();
                      else sink.error(result.cause());
                    }));
  }

  @Override
  public void dispose() {
    if (group != null) group.shutdownGracefully(0, 5, TimeUnit.SECONDS).syncUninterruptibly();
  }

  @Override
  public boolean isDisposed() {
    return group != null && group.isShuttingDown();
  }
}
