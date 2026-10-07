import dev.elide.bemo.transport.TransportNative;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;

/** Native V2 HTTP parser/encoder and optional Rustls/aws-lc-rs server. */
public final class NativeHttpBenchmarkServer {
  private static void require(boolean valid, String operation) {
    if (!valid) throw new IllegalStateException(operation);
  }

  private static long upload(TransportNative api, long owner, byte[] data) {
    long buffer = api.bufferNew(owner, Math.max(1, data.length));
    require(buffer != 0, "allocate input");
    api.bufferView(buffer).put(data);
    require(api.bufferFreeze(buffer, data.length) == 0, "freeze input");
    return buffer;
  }

  public static void run(TransportNative api, String[] args) throws Exception {
    boolean tls = Boolean.parseBoolean(args[3]);
    boolean gzip = Boolean.parseBoolean(args[4]);
    String gzipProvider = System.getenv().getOrDefault("BEMO_BENCH_GZIP_PROVIDER", "zlib-rs");
    require(
        gzipProvider.equals("zlib-rs") || gzipProvider.equals("java.util.zip"), "gzip provider");
    boolean javaGzip = gzipProvider.equals("java.util.zip");
    int gzipLevel =
        gzip ? Integer.parseInt(System.getenv().getOrDefault("BEMO_BENCH_GZIP_LEVEL", "1")) : 0;
    require(gzipLevel >= 0 && gzipLevel <= 9, "gzip level");
    int size = Integer.parseInt(args[5]);
    int backend = Integer.parseInt(args[6]);
    int socketBuffer =
        Integer.parseInt(System.getenv().getOrDefault("BEMO_BENCH_SOCKET_BUFFER", "0"));
    byte[] payload = new byte[size];
    byte[] pattern =
        "{\"message\":\"bemo transport benchmark\",\"value\":12345}\n"
            .getBytes(StandardCharsets.UTF_8);
    for (int i = 0; i < size; i++) payload[i] = pattern[i % pattern.length];
    long owner = api.ownerNew(64 * 1024 * 1024);
    require(owner != 0, "owner");
    long driver = api.driverNew(owner, backend, 256);
    require(driver != 0, "driver");
    List<Long> buffers = new ArrayList<>();
    Set<Long> connections = new HashSet<>();
    long context = 0;
    long listener = 0;
    AtomicBoolean running = new AtomicBoolean(true);
    long encoder = gzip && !javaGzip ? api.gzipNew(owner, gzipLevel) : 0;
    require(!gzip || javaGzip || encoder != 0, "native gzip state");
    try (ReusableGzip compressor = gzip && javaGzip ? new ReusableGzip(gzipLevel) : null) {
      if (tls) {
        long cert = upload(api, owner, Files.readAllBytes(Path.of(args[1])));
        long key = upload(api, owner, Files.readAllBytes(Path.of(args[2])));
        long alpn = upload(api, owner, "\010http/1.1".getBytes(StandardCharsets.US_ASCII));
        try {
          context = api.tlsServer(owner, cert, key, alpn);
          require(context != 0, "Rustls/aws-lc-rs server context");
        } finally {
          api.bufferRelease(cert);
          api.bufferRelease(key);
          api.bufferRelease(alpn);
        }
      }
      long endpoint = api.bufferNew(owner, 24);
      buffers.add(endpoint);
      api.bufferView(endpoint)
          .order(ByteOrder.nativeOrder())
          .put(0, (byte) 127)
          .put(3, (byte) 1)
          .putShort(18, (short) 4);
      require(api.bufferFreeze(endpoint, 24) == 0, "endpoint");
      listener = api.socketListen(owner, driver, endpoint, 128, 1);
      require(listener != 0, "listen");
      long address = api.bufferNew(owner, 24);
      buffers.add(address);
      require(api.socketAddress(driver, listener, 0, address) == 0, "bound address");
      int port =
          Short.toUnsignedInt(api.bufferView(address).order(ByteOrder.nativeOrder()).getShort(16));
      long body = upload(api, owner, payload);
      if (body != 0) buffers.add(body);
      long headerBytes =
          upload(
              api,
              owner,
              "content-typeapplication/jsoncontent-encodinggzip"
                  .getBytes(StandardCharsets.US_ASCII));
      buffers.add(headerBytes);
      long headers = api.bufferNew(owner, 64);
      buffers.add(headers);
      long textAddress = api.bufferAddress(headerBytes);
      api.bufferView(headers)
          .order(ByteOrder.nativeOrder())
          .putLong(0, textAddress)
          .putLong(8, 12)
          .putLong(16, textAddress + 12)
          .putLong(24, 16)
          .putLong(32, textAddress + 28)
          .putLong(40, 16)
          .putLong(48, textAddress + 44)
          .putLong(56, 4);
      long headersAddress = api.bufferAddress(headers);
      long tlsContext = context;
      long listenSocket = listener;
      TransportNative.EventCallback callback =
          (operation, socket, value, result, kind) -> {
            if (kind == 2) {
              require(result == 0 && value != 0, "accept completion");
              require(api.socketAdopt(owner, driver, value) == 0, "adopt");
              connections.add(value);
              require(api.socketOption(driver, value, 1, 1) == 0, "TCP_NODELAY");
              if (socketBuffer > 0) {
                require(
                    api.socketOption(driver, value, 3, socketBuffer) == 0, "receive socket buffer");
                require(
                    api.socketOption(driver, value, 4, socketBuffer) == 0, "send socket buffer");
              }
              require(
                  (tls
                          ? api.socketHttpTls(owner, driver, value, owner, 16 * 1024, tlsContext)
                          : api.socketHttp(owner, driver, value, owner, 16 * 1024))
                      == 0,
                  "V2 native HTTP");
              require(api.socketAccept(owner, driver, listenSocket) != 0, "rearm accept");
            } else if (kind == TransportNative.EVENT_REQUEST) {
              int length = payload.length;
              // Compress fresh input for every response; the native encoder never caches a member.
              long responseBody;
              if (gzip && !javaGzip) {
                responseBody = api.gzipCompress(owner, encoder, body);
                require(responseBody != 0, "native application gzip");
                length = api.bufferCapacity(responseBody);
              } else {
                if (gzip) length = compressor.compress(payload);
                responseBody = gzip ? api.bufferNew(owner, Math.max(1, length)) : body;
              }
              require(responseBody != 0, "response body allocation");
              try {
                if (gzip && javaGzip) {
                  compressor.put(api.bufferView(responseBody));
                  require(api.bufferFreeze(responseBody, length) == 0, "compressed body freeze");
                }
                require(
                    api.httpRespond(
                            driver,
                            value,
                            200,
                            headersAddress,
                            gzip ? 2 : 1,
                            0,
                            length,
                            TransportNative.RESPOND_STREAM)
                        == 0,
                    "native response head");
                require(
                    api.httpChunkSend(
                            driver,
                            value,
                            responseBody,
                            length,
                            TransportNative.CHUNK_FINAL | TransportNative.CHUNK_RETAIN)
                        == 0,
                    "retained response body");
                require(api.httpFree(driver, value) == 0, "exchange ownership");
              } finally {
                if (gzip) require(api.bufferRelease(responseBody) == 0, "compressed body handle");
              }
            } else if (kind == TransportNative.EVENT_CLOSED) {
              connections.remove(socket);
            } else if (result < 0) {
              throw new IllegalStateException("Native HTTP event " + kind + ": " + result);
            }
            return true;
          };
      require(api.socketAccept(owner, driver, listener) != 0, "initial accept");
      int selected = api.driverBackend(driver);
      long fallback = api.bufferNew(owner, 1024);
      buffers.add(fallback);
      int fallbackLength = api.driverFallback(driver, fallback);
      System.out.printf(
          "{\"port\":%d,\"driver\":\"%s\",\"auto_fallback\":%s,\"gzip_provider\":\"%s\",\"gzip_level\":%d}%n",
          port,
          switch (selected) {
            case 1 -> "polling";
            case 2 -> "io-uring";
            case 3 -> "iocp";
            default -> throw new IllegalStateException("Unknown backend " + selected);
          },
          fallbackLength > 0,
          gzip ? gzipProvider : "none",
          gzipLevel);
      System.out.flush();
      Thread stop =
          new Thread(
              () -> {
                try {
                  BufferedReader control =
                      new BufferedReader(
                          new InputStreamReader(System.in, StandardCharsets.US_ASCII));
                  String command;
                  while ((command = control.readLine()) != null) {
                    if (!command.equals("cpu")) break;
                    System.out.printf(
                        "{\"server_cpu_ns\":%d}%n",
                        ProcessHandle.current().info().totalCpuDuration().orElseThrow().toNanos());
                    System.out.flush();
                  }
                } catch (java.io.IOException failure) {
                  throw new IllegalStateException(failure);
                } finally {
                  running.set(false);
                  api.driverWake(driver);
                }
              },
              "benchmark-stop");
      stop.setDaemon(true);
      stop.start();
      long batch = api.bufferNew(owner, 40 * 256);
      buffers.add(batch);
      ByteBuffer events = api.bufferView(batch).order(ByteOrder.nativeOrder());
      while (running.get()) {
        if (api.supportsPollCallback()) {
          require(
              api.driverPollCallback(owner, driver, -1, 256, callback) >= 0, "HTTP callback poll");
        } else {
          int count = api.driverPoll(driver, -1, batch, 256);
          require(count >= 0, "HTTP poll");
          for (int i = 0; i < count; i++) {
            int offset = i * 40;
            callback.event(
                events.getLong(offset),
                events.getLong(offset + 8),
                events.getLong(offset + 16),
                events.getLong(offset + 24),
                events.getInt(offset + 32));
          }
        }
      }
    } finally {
      for (long socket : connections) api.socketClose(driver, socket);
      if (listener != 0) api.socketClose(driver, listener);
      api.httpRetire(driver);
      require(api.driverRelease(driver) == 0, "driver retirement");
      if (context != 0) api.tlsContextRelease(context);
      if (encoder != 0) require(api.gzipRelease(encoder) == 0, "gzip state retirement");
      for (long buffer : buffers) api.bufferRelease(buffer);
      require(api.ownerUsed(owner) == 0, "native storage reclaimed");
      require(api.ownerRelease(owner) == 0, "owner retirement");
    }
  }
}
