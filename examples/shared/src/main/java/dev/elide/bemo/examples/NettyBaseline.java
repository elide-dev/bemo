package dev.elide.bemo.examples;

import io.netty.channel.Channel;
import io.netty.channel.IoHandlerFactory;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollIoHandler;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.epoll.EpollSocketChannel;
import io.netty.channel.kqueue.KQueue;
import io.netty.channel.kqueue.KQueueIoHandler;
import io.netty.channel.kqueue.KQueueServerSocketChannel;
import io.netty.channel.kqueue.KQueueSocketChannel;
import io.netty.channel.socket.ServerSocketChannel;
import io.netty.channel.socket.SocketChannel;
import io.netty.util.internal.PlatformDependent;

/** Explicit native comparison transport; unavailable backends must fail rather than use NIO. */
public final class NettyBaseline {
  private NettyBaseline() {}

  public static String transport() {
    if (PlatformDependent.isOsx()) {
      KQueue.ensureAvailability();
      return "kqueue";
    }
    if (PlatformDependent.normalizedOs().equals("linux")) {
      Epoll.ensureAvailability();
      return "epoll";
    }
    throw new UnsupportedOperationException(
        "Netty comparison requires Linux epoll or macOS kqueue");
  }

  public static IoHandlerFactory factory() {
    String transport = transport();
    System.out.println("Netty native transport enabled (" + transport + ")");
    return transport.equals("epoll") ? EpollIoHandler.newFactory() : KQueueIoHandler.newFactory();
  }

  public static Class<? extends ServerSocketChannel> serverClass() {
    return transport().equals("epoll")
        ? EpollServerSocketChannel.class
        : KQueueServerSocketChannel.class;
  }

  public static Class<? extends SocketChannel> socketClass() {
    return transport().equals("epoll") ? EpollSocketChannel.class : KQueueSocketChannel.class;
  }

  public static <C extends Channel> C channel(Class<C> type) {
    if (type == ServerSocketChannel.class) {
      return type.cast(
          transport().equals("epoll")
              ? new EpollServerSocketChannel()
              : new KQueueServerSocketChannel());
    }
    if (type == SocketChannel.class) {
      return type.cast(
          transport().equals("epoll") ? new EpollSocketChannel() : new KQueueSocketChannel());
    }
    throw new UnsupportedOperationException("Netty comparison supports TCP only: " + type);
  }

  public static void verifyChannel(Channel channel) {
    if (!serverClass().isInstance(channel) && !socketClass().isInstance(channel)) {
      throw new IllegalStateException(
          "Unexpected comparison channel: " + channel.getClass().getName());
    }
    System.out.println("Netty native channel verified (" + channel.getClass().getName() + ")");
  }
}
