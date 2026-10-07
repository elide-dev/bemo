package dev.elide.bemo.examples.micronaut;

import dev.elide.bemo.examples.BenchmarkCompression;
import io.micronaut.context.annotation.Context;
import io.micronaut.http.netty.channel.EventLoopGroupRegistry;
import io.netty.channel.EventLoopGroup;
import jakarta.annotation.PreDestroy;

/** Reclaim compressors on their owner threads before Micronaut destroys the loop registry. */
@Context
public final class CompressionLifecycle {
  private final EventLoopGroup group;

  public CompressionLifecycle(EventLoopGroupRegistry registry) {
    group = registry.getDefaultEventLoopGroup();
  }

  @PreDestroy
  void close() {
    for (var loop : group) {
      if (loop.inEventLoop()) BenchmarkCompression.releaseCurrentThread();
      else loop.submit(BenchmarkCompression::releaseCurrentThread).syncUninterruptibly();
    }
  }
}
