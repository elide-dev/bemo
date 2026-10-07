import dev.elide.bemo.transport.TransportNative;
import java.nio.ByteBuffer;

/** Enforces the requested backend on every driver in either binding's contract suite. */
class BackendTransport implements TransportNative {
  @Override
  public long gzipNew(long workload, int level) {
    return delegate.gzipNew(workload, level);
  }

  @Override
  public long gzipCompress(long workload, long encoder, long input) {
    return delegate.gzipCompress(workload, encoder, input);
  }

  @Override
  public int gzipRelease(long encoder) {
    return delegate.gzipRelease(encoder);
  }

  @Override
  public long servingNew(int contexts) {
    return delegate.servingNew(contexts);
  }

  @Override
  public long servingSplitNew(int contexts) {
    return delegate.servingSplitNew(contexts);
  }

  @Override
  public int servingWorkers(long application, int context) {
    return delegate.servingWorkers(application, context);
  }

  @Override
  public long servingWorkerDriver(
      long workload, long application, int context, int worker, int backend, int limit) {
    return delegate.servingWorkerDriver(workload, application, context, worker, requested, limit);
  }

  @Override
  public long servingDriver(long workload, long application, int replica, int backend, int limit) {
    return delegate.servingDriver(workload, application, replica, requested, limit);
  }

  @Override
  public long servingListen(
      long workload, long driver, long endpoint, long signature, int backlog) {
    return delegate.servingListen(workload, driver, endpoint, signature, backlog);
  }

  @Override
  public int servingReady(long driver) {
    return delegate.servingReady(driver);
  }

  @Override
  public int servingCpu(long driver) {
    return delegate.servingCpu(driver);
  }

  @Override
  public int servingListenerClose(long driver, long listener) {
    return delegate.servingListenerClose(driver, listener);
  }

  @Override
  public int servingClose(long application) {
    return delegate.servingClose(application);
  }

  private final TransportNative delegate;
  private final int requested;

  BackendTransport(TransportNative delegate) {
    this.delegate = delegate;
    String name = System.getenv("ELIDE_TRANSPORT_TEST_BACKEND");
    requested =
        switch (name == null ? "auto" : name) {
          case "auto" -> 0;
          case "kqueue" -> {
            if (!System.getProperty("os.name").startsWith("Mac"))
              throw new AssertionError("kqueue requires macOS");
            yield 1;
          }
          case "epoll" -> {
            if (!System.getProperty("os.name").equals("Linux"))
              throw new AssertionError("epoll requires Linux");
            yield 1;
          }
          case "io-uring" -> {
            if (!System.getProperty("os.name").equals("Linux"))
              throw new AssertionError("io-uring requires Linux");
            yield 2;
          }
          case "iocp" -> {
            if (!System.getProperty("os.name").startsWith("Windows"))
              throw new AssertionError("iocp requires Windows");
            yield 3;
          }
          default -> throw new AssertionError("Unknown backend: " + name);
        };
  }

  @Override
  public long driverNew(long workload, int backend, int limit) {
    long driver = delegate.driverNew(workload, requested, limit);
    if (driver == 0) throw new AssertionError("Requested backend unavailable: " + requested);
    int actual = delegate.driverBackend(driver);
    if (actual <= 0 || (requested != 0 && actual != requested)) {
      delegate.driverRelease(driver);
      throw new AssertionError("Backend mismatch: requested " + requested + ", actual " + actual);
    }
    String selected =
        switch (actual) {
          case 1 -> System.getProperty("os.name").startsWith("Mac") ? "kqueue" : "epoll";
          case 2 -> "io-uring";
          case 3 -> "iocp";
          default -> throw new AssertionError("Unknown native backend: " + actual);
        };
    System.out.println("Verified native backend: " + selected + " (requested " + requested + ")");
    return driver;
  }

  @Override
  public int version() {
    return delegate.version();
  }

  @Override
  public int lastError() {
    return delegate.lastError();
  }

  @Override
  public long ownerNew(long limit) {
    return delegate.ownerNew(limit);
  }

  @Override
  public long ownerUsed(long owner) {
    return delegate.ownerUsed(owner);
  }

  @Override
  public int ownerRelease(long owner) {
    return delegate.ownerRelease(owner);
  }

  @Override
  public int workloadClose(long workload) {
    return delegate.workloadClose(workload);
  }

  @Override
  public long bufferNew(long owner, long capacity) {
    return delegate.bufferNew(owner, capacity);
  }

  @Override
  public ByteBuffer bufferView(long buffer) {
    return delegate.bufferView(buffer);
  }

  @Override
  public int bufferCapacity(long buffer) {
    return delegate.bufferCapacity(buffer);
  }

  @Override
  public int bufferFreeze(long buffer, long length) {
    return delegate.bufferFreeze(buffer, length);
  }

  @Override
  public long bufferSlice(long buffer, long offset, long length) {
    return delegate.bufferSlice(buffer, offset, length);
  }

  @Override
  public int bufferRelease(long buffer) {
    return delegate.bufferRelease(buffer);
  }

  @Override
  public int driverBackend(long driver) {
    return delegate.driverBackend(driver);
  }

  @Override
  public int driverWake(long driver) {
    return delegate.driverWake(driver);
  }

  @Override
  public int driverFallback(long driver, long output) {
    return delegate.driverFallback(driver, output);
  }

  @Override
  public int driverRelease(long driver) {
    return delegate.driverRelease(driver);
  }

  @Override
  public long socketListen(long workload, long driver, long endpoint, int backlog, int reuse) {
    return delegate.socketListen(workload, driver, endpoint, backlog, reuse);
  }

  @Override
  public long socketConnect(long workload, long driver, long endpoint) {
    return delegate.socketConnect(workload, driver, endpoint);
  }

  @Override
  public long socketAccept(long workload, long driver, long listener) {
    return delegate.socketAccept(workload, driver, listener);
  }

  @Override
  public int socketAdopt(long workload, long driver, long socket) {
    return delegate.socketAdopt(workload, driver, socket);
  }

  @Override
  public int socketDiscard(long socket) {
    return delegate.socketDiscard(socket);
  }

  @Override
  public int socketAddress(long driver, long socket, int peer, long output) {
    return delegate.socketAddress(driver, socket, peer, output);
  }

  @Override
  public long socketReceive(long workload, long driver, long socket, long buffer) {
    return delegate.socketReceive(workload, driver, socket, buffer);
  }

  @Override
  public long socketReceiveNew(long workload, long driver, long socket, long owner, long capacity) {
    return delegate.socketReceiveNew(workload, driver, socket, owner, capacity);
  }

  @Override
  public boolean supportsInlineWrites() {
    return delegate.supportsInlineWrites();
  }

  @Override
  public long socketSendInline(long workload, long driver, long socket, ByteBuffer source) {
    return delegate.socketSendInline(workload, driver, socket, source);
  }

  @Override
  public boolean supportsGatheredWrites() {
    return delegate.supportsGatheredWrites();
  }

  @Override
  public long socketSendGathered(
      long workload, long driver, long socket, long[] regions, int count) {
    return delegate.socketSendGathered(workload, driver, socket, regions, count);
  }

  @Override
  public boolean supportsReceiveResults() {
    return delegate.supportsReceiveResults();
  }

  @Override
  public ReceiveResult socketReceiveNewResult(
      long workload, long driver, long socket, long owner, long capacity) {
    return delegate.socketReceiveNewResult(workload, driver, socket, owner, capacity);
  }

  @Override
  public long socketSend(
      long workload, long driver, long socket, long buffer, long offset, long length) {
    return delegate.socketSend(workload, driver, socket, buffer, offset, length);
  }

  @Override
  public int socketClose(long driver, long socket) {
    return delegate.socketClose(driver, socket);
  }

  @Override
  public int driverPoll(long driver, long timeoutNanos, long batch, int maximum) {
    return delegate.driverPoll(driver, timeoutNanos, batch, maximum);
  }

  @Override
  public int socketOption(long driver, long socket, int option, int value) {
    return delegate.socketOption(driver, socket, option, value);
  }

  @Override
  public int socketShutdown(long driver, long socket, int direction) {
    return delegate.socketShutdown(driver, socket, direction);
  }

  @Override
  public long tlsClient(long workload, long roots, long alpn) {
    return delegate.tlsClient(workload, roots, alpn);
  }

  @Override
  public long tlsServer(long workload, long chain, long key, long alpn) {
    return delegate.tlsServer(workload, chain, key, alpn);
  }

  @Override
  public int tlsContextRelease(long context) {
    return delegate.tlsContextRelease(context);
  }

  @Override
  public long tlsNew(long workload, long context, long owner, long name) {
    return delegate.tlsNew(workload, context, owner, name);
  }

  @Override
  public int tlsFeed(long session, long input, long length) {
    return delegate.tlsFeed(session, input, length);
  }

  @Override
  public int tlsStep(
      long session, int action, long plaintext, long offset, long length, long output) {
    return delegate.tlsStep(session, action, plaintext, offset, length, output);
  }

  @Override
  public int tlsProtocol(long session, long output) {
    return delegate.tlsProtocol(session, output);
  }

  @Override
  public int tlsRelease(long session) {
    return delegate.tlsRelease(session);
  }

  @Override
  public int socketHttp(long workload, long driver, long socket, long owner, long capacity) {
    return delegate.socketHttp(workload, driver, socket, owner, capacity);
  }

  @Override
  public int socketHttpTls(
      long workload, long driver, long socket, long owner, long capacity, long context) {
    return delegate.socketHttpTls(workload, driver, socket, owner, capacity, context);
  }

  @Override
  public int httpMethod(long exchange) {
    return delegate.httpMethod(exchange);
  }

  @Override
  public int httpVersion(long exchange) {
    return delegate.httpVersion(exchange);
  }

  @Override
  public int httpHeaderCount(long exchange) {
    return delegate.httpHeaderCount(exchange);
  }

  @Override
  public int httpKeepAlive(long exchange) {
    return delegate.httpKeepAlive(exchange);
  }

  @Override
  public int httpView(long exchange, int kind, int index, long output) {
    return delegate.httpView(exchange, kind, index, output);
  }

  @Override
  public int httpRespond(
      long driver,
      long exchange,
      int status,
      long headers,
      int count,
      long body,
      long bodyLength,
      int flags) {
    return delegate.httpRespond(driver, exchange, status, headers, count, body, bodyLength, flags);
  }

  @Override
  public int httpRelease(long driver, long exchange) {
    return delegate.httpRelease(driver, exchange);
  }

  @Override
  public int httpSpans(long exchange, long output, int capacity) {
    return delegate.httpSpans(exchange, output, capacity);
  }

  @Override
  public long httpRetain(long exchange, long lengthOut) {
    return delegate.httpRetain(exchange, lengthOut);
  }

  @Override
  public int httpHeadRelease(long head) {
    return delegate.httpHeadRelease(head);
  }

  @Override
  public int httpDrain(long driver) {
    return delegate.httpDrain(driver);
  }

  @Override
  public int httpRetire(long driver) {
    return delegate.httpRetire(driver);
  }

  @Override
  public int httpFree(long driver, long exchange) {
    return delegate.httpFree(driver, exchange);
  }

  @Override
  public long httpPrepare(long exchange, long capacity) {
    return delegate.httpPrepare(exchange, capacity);
  }

  @Override
  public int httpSend(long driver, long exchange, long length, int flags) {
    return delegate.httpSend(driver, exchange, length, flags);
  }

  @Override
  public int httpSegmentAck(long driver, long segment) {
    return delegate.httpSegmentAck(driver, segment);
  }

  @Override
  public int httpSegmentRelease(long driver, long segment) {
    return delegate.httpSegmentRelease(driver, segment);
  }

  @Override
  public int httpSegmentReleaseRetired(long segment) {
    return delegate.httpSegmentReleaseRetired(segment);
  }

  @Override
  public long httpChunkPrepare(long exchange, long capacity, long addressOut) {
    return delegate.httpChunkPrepare(exchange, capacity, addressOut);
  }

  @Override
  public int httpChunkSend(long driver, long exchange, long buffer, long length, int flags) {
    return delegate.httpChunkSend(driver, exchange, buffer, length, flags);
  }

  @Override
  public ByteBuffer memory(long address, int length) {
    return delegate.memory(address, length);
  }

  @Override
  public long bufferAddress(long buffer) {
    return delegate.bufferAddress(buffer);
  }

  @Override
  public long getLong(long address) {
    return delegate.getLong(address);
  }

  @Override
  public int getInt(long address) {
    return delegate.getInt(address);
  }

  @Override
  public byte getByte(long address) {
    return delegate.getByte(address);
  }

  @Override
  public byte[] bytes(long address, int length) {
    return delegate.bytes(address, length);
  }

  // ---- SSLEngine (begin) ----
  @Override
  public boolean supportsEngine() {
    return delegate.supportsEngine();
  }

  @Override
  public long engineContextNew(
      long workload, int flags, byte[] certificates, byte[] key, byte[] alpn) {
    return delegate.engineContextNew(workload, flags, certificates, key, alpn);
  }

  @Override
  public int engineContextRelease(long context) {
    return delegate.engineContextRelease(context);
  }

  @Override
  public long engineNew(long workload, long context, byte[] name) {
    return delegate.engineNew(workload, context, name);
  }

  @Override
  public long engineWrap(
      long engine,
      ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    return delegate.engineWrap(engine, source, sourceLength, destination, destinationLength);
  }

  @Override
  public long engineUnwrap(
      long engine,
      ByteBuffer source,
      int sourceLength,
      ByteBuffer destination,
      int destinationLength) {
    return delegate.engineUnwrap(engine, source, sourceLength, destination, destinationLength);
  }

  @Override
  public long engineControl(long engine, int operation) {
    return delegate.engineControl(engine, operation);
  }

  @Override
  public int engineInfo(long engine, int kind, int index, byte[] output) {
    return delegate.engineInfo(engine, kind, index, output);
  }

  @Override
  public int engineRelease(long engine) {
    return delegate.engineRelease(engine);
  }
  // ---- SSLEngine (end) ----
}
