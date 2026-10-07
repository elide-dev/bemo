import dev.elide.bemo.transport.TransportNative;
import java.io.ByteArrayOutputStream;
import java.net.InetAddress;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/** Shared FFM/C API retained HTTP body ownership and pipeline contract. */
public final class NativeHttpBodyTest {
  private static void check(boolean valid, String operation) {
    if (!valid) throw new AssertionError(operation);
  }

  public static void verify(TransportNative api) throws Exception {
    long owner = api.ownerNew(1024 * 1024);
    long bodyOwner = api.ownerNew(16);
    long body = api.bufferNew(bodyOwner, 16);
    api.bufferView(body).put("body".getBytes(StandardCharsets.US_ASCII));
    check(api.bufferFreeze(body, 4) == 0, "frozen body");
    long driver = api.driverNew(owner, 0, 32);
    long endpoint = api.bufferNew(owner, 24);
    api.bufferView(endpoint)
        .order(ByteOrder.nativeOrder())
        .put(0, (byte) 127)
        .put(3, (byte) 1)
        .putShort(18, (short) 4);
    check(api.bufferFreeze(endpoint, 24) == 0, "endpoint");
    long listener = api.socketListen(owner, driver, endpoint, 16, 1);
    check(listener != 0, "listener");
    api.bufferRelease(endpoint);
    long address = api.bufferNew(owner, 24);
    check(api.socketAddress(driver, listener, 0, address) == 0, "bound address");
    int port =
        Short.toUnsignedInt(api.bufferView(address).order(ByteOrder.nativeOrder()).getShort(16));
    api.bufferRelease(address);
    long batch = api.bufferNew(owner, 40 * 32);
    ByteBuffer events = api.bufferView(batch).order(ByteOrder.nativeOrder());
    long server = 0;
    long[] exchanges = new long[2];
    int requests = 0;

    long deadline = System.nanoTime() + 10_000_000_000L;
    check(api.socketAccept(owner, driver, listener) != 0, "accept");
    try (Socket client = new Socket(InetAddress.getByAddress(new byte[] {127, 0, 0, 1}), port)) {
      client.setSoTimeout(50);
      client
          .getOutputStream()
          .write(
              ("GET /one HTTP/1.1\r\nHost: a\r\n\r\n" + "GET /two HTTP/1.1\r\nHost: a\r\n\r\n")
                  .getBytes(StandardCharsets.US_ASCII));
      while (requests < 2) {
        check(System.nanoTime() < deadline, "request deadline");
        int count = api.driverPoll(driver, 10_000_000L, batch, 32);
        check(count >= 0, "request poll");
        for (int i = 0; i < count; i++) {
          int offset = i * 40;
          int kind = events.getInt(offset + 32);
          if (kind == 2) {
            server = events.getLong(offset + 16);
            check(api.socketAdopt(owner, driver, server) == 0, "adopt");
            check(api.socketHttp(owner, driver, server, owner, 16384) == 0, "HTTP mode");
          } else if (kind == TransportNative.EVENT_REQUEST) {
            check(requests < 2, "extra request");
            exchanges[requests++] = events.getLong(offset + 16);
          }
        }
      }
      for (long exchange : exchanges) {
        check(
            api.httpRespond(driver, exchange, 200, 0, 0, 0, 4, TransportNative.RESPOND_STREAM) == 0,
            "head");
        check(
            api.httpChunkSend(driver, exchange, body, 17, TransportNative.CHUNK_RETAIN) == -1,
            "initialized length bound");
        check(
            api.httpChunkSend(
                    driver,
                    exchange,
                    body,
                    4,
                    TransportNative.CHUNK_RETAIN | TransportNative.CHUNK_FINAL)
                == 0,
            "retained send");
        check(api.bufferCapacity(body) == 4, "caller keeps immutable handle");
        check(api.httpFree(driver, exchange) == 0, "early exchange free");
      }
      check(api.bufferRelease(body) == 0, "early caller release");
      check(api.ownerUsed(bodyOwner) == 16, "queued sends retain storage");
      ByteArrayOutputStream wire = new ByteArrayOutputStream();
      byte[] input = new byte[4096];
      while (api.ownerUsed(bodyOwner) != 0
          || wire.toString(StandardCharsets.US_ASCII).split("\r\n\r\nbody", -1).length != 3) {
        check(System.nanoTime() < deadline, "send deadline");
        int count = api.driverPoll(driver, 10_000_000L, batch, 32);
        check(count >= 0, "send poll");
        for (int i = 0; i < count; i++) {
          int offset = i * 40;
          if (events.getInt(offset + 32) == TransportNative.EVENT_PART_SENT) {
            check(events.getLong(offset + 24) > 0, "send completion");
            throw new AssertionError("freed exchange reports a completion");
          }
        }
        try {
          int countRead = client.getInputStream().read(input);
          check(countRead > 0, "unexpected EOF");
          wire.write(input, 0, countRead);
        } catch (SocketTimeoutException expected) {
          // Drive remaining native writes/completions before reading again.
        }
      }
      String text = wire.toString(StandardCharsets.US_ASCII);
      check(text.split("HTTP/1.1 200", -1).length == 3, "two ordered responses");
      check(text.split("\r\n\r\nbody", -1).length == 3, "exact retained bodies");
      check(text.endsWith("body"), "complete response");
    }
    api.socketClose(driver, server);
    api.socketClose(driver, listener);
    api.httpRetire(driver);
    int retired;
    while ((retired = api.driverRelease(driver)) == -2) {
      check(System.nanoTime() < deadline, "driver retirement deadline");
      check(api.driverPoll(driver, 1_000_000L, batch, 32) >= 0, "retirement poll");
    }
    check(retired == 0, "driver retirement");
    api.bufferRelease(batch);
    check(api.ownerUsed(owner) == 0 && api.ownerUsed(bodyOwner) == 0, "all storage reclaimed");
    api.ownerRelease(bodyOwner);
    api.ownerRelease(owner);
  }
}
