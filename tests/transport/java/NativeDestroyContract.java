package dev.elide.dokar.transport;

import io.netty.buffer.ByteBuf;
import io.netty.channel.IoEvent;
import io.netty.channel.IoRegistration;
import io.netty.util.concurrent.ThreadAwareExecutor;

/** Reproduces destruction without a preceding prepare turn, without racing the scheduler. */
public final class NativeDestroyContract {

  public static void verify(TransportNative api) throws Exception {
    Thread owner = Thread.currentThread();
    ThreadAwareExecutor executor =
        new ThreadAwareExecutor() {
          @Override
          public boolean isExecutorThread(Thread thread) {
            return thread == owner;
          }

          @Override
          public void execute(Runnable task) {
            if (Thread.currentThread() != owner) throw new AssertionError("wrong owner");
            task.run();
          }
        };
    NativeIoHandler handler =
        (NativeIoHandler) NativeIoHandler.newFactory(api, 0, 8, 65536).newHandler(executor);
    handler.initialize();
    ByteBuf retained = handler.allocator.directBuffer(16384, 16384);
    int[] closes = {0};
    handler.register(
        new NativeIoHandler.Handle() {
          @Override
          public void handle(IoRegistration registration, IoEvent event) {
            throw new AssertionError("unexpected completion");
          }

          @Override
          public void close() {
            if (++closes[0] != 1) throw new AssertionError("duplicate close");
            if (api.driverBackend(handler.driver()) <= 0)
              throw new AssertionError("driver released before handle");
            retained.release();
          }
        });
    handler.destroy();
    if (closes[0] != 1 || retained.refCnt() != 0)
      throw new AssertionError("destroy skipped handle cleanup");
    handler.destroy();
    if (closes[0] != 1) throw new AssertionError("destroy was not idempotent");
  }
}
