package dev.elide.bemo.examples.micronaut;

import dev.elide.bemo.examples.BemoRuntime;
import dev.elide.bemo.examples.BenchmarkPayload;
import dev.elide.bemo.examples.NettyBaseline;
import dev.elide.bemo.transport.NativeIoHandler;
import dev.elide.bemo.transport.NativeServerSocketChannel;
import dev.elide.bemo.transport.NativeSocketChannel;
import dev.elide.bemo.transport.TransportNative;
import io.micronaut.context.annotation.Primary;
import io.micronaut.context.annotation.Replaces;
import io.micronaut.http.netty.channel.DefaultEventLoopGroupFactory;
import io.micronaut.http.netty.channel.EventLoopGroupConfiguration;
import io.micronaut.http.netty.channel.EventLoopGroupFactory;
import io.micronaut.http.netty.channel.NettyChannelType;
import io.netty.channel.Channel;
import io.netty.channel.IoHandlerFactory;
import jakarta.inject.Singleton;

/** Micronaut owns loop shutdown; select Bemo or the required native Netty baseline. */
@Singleton
@Primary
@Replaces(DefaultEventLoopGroupFactory.class)
public final class BemoEventLoopGroupFactory implements EventLoopGroupFactory {
  // Acceptor and worker groups must share one binding for socket ownership transfer.
  private final TransportNative api = BenchmarkPayload.bemoEnabled() ? BemoRuntime.create() : null;

  @Override
  public IoHandlerFactory createIoHandlerFactory(EventLoopGroupConfiguration configuration) {
    if (api == null) return NettyBaseline.factory();
    return NativeIoHandler.newFactory(api, 0, 256, 64 * 1024 * 1024);
  }

  @Override
  public Class<? extends Channel> channelClass(NettyChannelType type) {
    if (api == null)
      return switch (type) {
        case SERVER_SOCKET -> NettyBaseline.serverClass();
        case CLIENT_SOCKET -> NettyBaseline.socketClass();
        default ->
            throw new UnsupportedOperationException("Netty comparison supports TCP only: " + type);
      };
    return switch (type) {
      case SERVER_SOCKET -> NativeServerSocketChannel.class;
      case CLIENT_SOCKET -> NativeSocketChannel.class;
      default -> throw new UnsupportedOperationException("Bemo example supports TCP only: " + type);
    };
  }

  @Override
  public Class<? extends Channel> channelClass(
      NettyChannelType type, EventLoopGroupConfiguration configuration) {
    return channelClass(type);
  }

  @Override
  public Channel channelInstance(NettyChannelType type, EventLoopGroupConfiguration configuration) {
    if (api == null) {
      Channel channel =
          switch (type) {
            case SERVER_SOCKET ->
                NettyBaseline.channel(io.netty.channel.socket.ServerSocketChannel.class);
            case CLIENT_SOCKET ->
                NettyBaseline.channel(io.netty.channel.socket.SocketChannel.class);
            default ->
                throw new UnsupportedOperationException(
                    "Netty comparison supports TCP only: " + type);
          };
      NettyBaseline.verifyChannel(channel);
      return channel;
    }
    return switch (type) {
      case SERVER_SOCKET -> new NativeServerSocketChannel();
      case CLIENT_SOCKET -> new NativeSocketChannel();
      default -> throw new UnsupportedOperationException("Bemo example supports TCP only: " + type);
    };
  }
}
