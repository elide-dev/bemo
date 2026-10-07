package dev.elide.bemo.examples.micronaut;

import dev.elide.bemo.examples.BemoRuntime;
import dev.elide.bemo.transport.NativeIoHandler;
import dev.elide.bemo.transport.NativeServerSocketChannel;
import dev.elide.bemo.transport.NativeSocketChannel;
import dev.elide.bemo.transport.TransportNative;
import io.micronaut.context.annotation.Primary;
import io.micronaut.context.annotation.Replaces;
import io.micronaut.context.annotation.Requires;
import io.micronaut.http.netty.channel.DefaultEventLoopGroupFactory;
import io.micronaut.http.netty.channel.EventLoopGroupConfiguration;
import io.micronaut.http.netty.channel.EventLoopGroupFactory;
import io.micronaut.http.netty.channel.NettyChannelType;
import io.netty.channel.Channel;
import io.netty.channel.IoHandlerFactory;
import jakarta.inject.Singleton;

/** Micronaut owns loop shutdown; Bemo supplies the I/O handler and TCP channels. */
@Singleton
@Primary
@Replaces(DefaultEventLoopGroupFactory.class)
@Requires(property = "bemo.enabled", value = "true", defaultValue = "true")
public final class BemoEventLoopGroupFactory implements EventLoopGroupFactory {
  // Acceptor and worker groups must share one binding for socket ownership transfer.
  private final TransportNative api = BemoRuntime.create();

  @Override
  public IoHandlerFactory createIoHandlerFactory(EventLoopGroupConfiguration configuration) {
    return NativeIoHandler.newFactory(api, 0, 256, 64 * 1024 * 1024);
  }

  @Override
  public Class<? extends Channel> channelClass(NettyChannelType type) {
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
    return switch (type) {
      case SERVER_SOCKET -> new NativeServerSocketChannel();
      case CLIENT_SOCKET -> new NativeSocketChannel();
      default -> throw new UnsupportedOperationException("Bemo example supports TCP only: " + type);
    };
  }
}
