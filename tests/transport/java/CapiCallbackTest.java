import dev.elide.netty.v2.TransportNative;
import dev.elide.netty.v2.svm.CapiTransportNative;
import java.net.Socket;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/** Real Native Image upcalls, including response reentry and exception containment. */
public final class CapiCallbackTest {

  static void check(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }

  static void verify() throws Exception {
    var api = new CapiTransportNative();
    long owner = api.ownerNew(4 * 1024 * 1024);
    long driver =
        api.driverNew(
            owner, "io-uring".equals(System.getenv("ELIDE_TRANSPORT_TEST_BACKEND")) ? 2 : 0, 32);
    long address = api.bufferNew(owner, 24);
    api.bufferView(address)
        .order(ByteOrder.nativeOrder())
        .put(0, (byte) 127)
        .put(3, (byte) 1)
        .putShort(18, (short) 4);
    api.bufferFreeze(address, 24);
    long listener = api.socketListen(owner, driver, address, 16, 1);
    api.bufferRelease(address);
    address = api.bufferNew(owner, 24);
    check(api.socketAddress(driver, listener, 0, address) == 0, "listener address");
    int port =
        Short.toUnsignedInt(api.bufferView(address).order(ByteOrder.nativeOrder()).getShort(16));
    api.bufferRelease(address);
    long batch = api.bufferNew(owner, 40);
    long body = api.bufferNew(owner, 8);
    api.bufferView(body).put("callback".getBytes(StandardCharsets.US_ASCII));
    long bodyAddress = api.bufferAddress(body);
    long[] connection = {0};
    int[] requests = {0};
    Thread ownerThread = Thread.currentThread();
    RuntimeException expected = new IllegalStateException("callback failure");
    check(api.socketAccept(owner, driver, listener) != 0, "accept");
    try (var peer = new Socket("127.0.0.1", port)) {
      peer.setSoTimeout(5000);
      peer.getOutputStream()
          .write(
              ("GET /1 HTTP/1.1\r\nHost: test\r\n\r\n" + "GET /2 HTTP/1.1\r\nHost: test\r\n\r\n")
                  .getBytes(StandardCharsets.US_ASCII));
      TransportNative.EventCallback callback =
          (operation, socket, value, result, kind) -> {
            check(Thread.currentThread() == ownerThread, "owner thread");
            check(api.driverPoll(driver, 0, batch, 1) == -1, "recursive poll rejected");
            if (kind == 2) {
              check(result == 0, "accept result");
              connection[0] = value;
              check(api.socketAdopt(owner, driver, value) == 0, "adopt during callback");
              check(
                  api.socketHttp(owner, driver, value, owner, 16384) == 0, "HTTP during callback");
            } else if (kind == TransportNative.EVENT_REQUEST) {
              check(
                  api.httpRespond(driver, value, 200, 0, 0, bodyAddress, 8, 0) == 0,
                  "response reentry");
              check(api.httpFree(driver, value) == 0, "free reentry");
              if (++requests[0] == 1) throw expected;
            }
            return true;
          };
      boolean caught = false;
      long deadline = System.nanoTime() + 5_000_000_000L;
      while (requests[0] < 2 && System.nanoTime() < deadline) {
        try {
          check(
              api.driverPollCallback(owner, driver, 50_000_000, 8, callback) >= 0, "callback poll");
        } catch (RuntimeException failure) {
          check(failure == expected, "same exception returns after native unwind");
          caught = true;
        }
      }
      check(caught && requests[0] == 2, "remaining pipeline survives callback failure");
      for (int i = 0; i < 4; i++) api.driverPollCallback(owner, driver, 10_000_000, 8, callback);
      byte[] wire = new byte[4096];
      int count = peer.getInputStream().read(wire);
      String text = new String(wire, 0, count, StandardCharsets.US_ASCII);
      while (text.split("callback", -1).length < 3) {
        count = peer.getInputStream().read(wire);
        check(count > 0, "two response bodies");
        text += new String(wire, 0, count, StandardCharsets.US_ASCII);
      }
      check(text.startsWith("HTTP/1.1 200"), "wire response");
    } finally {
      api.socketClose(driver, connection[0]);
      api.socketClose(driver, listener);
      for (int i = 0; i < 4; i++)
        api.driverPollCallback(owner, driver, 10_000_000, 8, (a, b, c, d, e) -> true);
      api.bufferRelease(batch);
      api.bufferRelease(body);
      check(api.driverRelease(driver) == 0, "driver cleanup");
      check(api.ownerUsed(owner) == 0, "callback storage reclaimed");
      api.ownerRelease(owner);
    }
    System.out.println("Native callback checks passed");
  }
}
