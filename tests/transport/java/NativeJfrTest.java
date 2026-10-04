import dev.elide.dokar.transport.*;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import jdk.jfr.Recording;
import jdk.jfr.consumer.RecordedEvent;
import jdk.jfr.consumer.RecordingFile;

/** Read actual recordings; assertions concern causal operations, not timing or batch boundaries. */
public final class NativeJfrTest {

  private static final List<String> TYPES =
      List.of(
          "dev.elide.TransportBatch",
          "dev.elide.TransportTlsHandshake",
          "dev.elide.TransportCopy",
          "dev.elide.TransportShutdown");

  public static void main(String[] args) throws Exception {
    verify(
        new BackendTransport(new FfmTransportNative(Path.of(args[0]))),
        Files.readAllBytes(Path.of(args[1])),
        Files.readAllBytes(Path.of(args[2])));
  }

  public static void verify(TransportNative api, byte[] cert, byte[] key) throws Exception {
    Path file = Files.createTempFile("native-transport-", ".jfr");
    try {
      try (Recording recording = new Recording()) {
        for (String type : TYPES) recording.disable(type);
        recording.start();
        NativeChannelTest.verify(api);
        recording.stop();
        recording.dump(file);
      }
      if (RecordingFile.readAllEvents(file).stream()
          .anyMatch(event -> TYPES.contains(event.getEventType().getName())))
        throw new AssertionError("Disabled transport events were recorded");
      try (Recording recording = new Recording()) {
        for (String type : TYPES) recording.enable(type).withThreshold(Duration.ZERO);
        recording.start();
        NativeChannelTest.verify(api);
        NativeTlsChannelTest.verify(api, cert, key);
        recording.stop();
        recording.dump(file);
      }
      List<RecordedEvent> events = RecordingFile.readAllEvents(file);
      Set<Long> drivers = new HashSet<>();
      Set<String> channels = new HashSet<>();
      Set<String> copies = new HashSet<>();
      long read = 0;
      long written = 0;
      int handshakes = 0;
      int shutdowns = 0;
      for (RecordedEvent event : events) {
        switch (event.getEventType().getName()) {
          case "dev.elide.TransportBatch" -> {
            drivers.add(event.getLong("driverId"));
            if (event.getString("backend").equals("unknown"))
              throw new AssertionError("Missing actual backend");
            int count = event.getInt("completions");
            if (count < 0
                || count > event.getInt("capacity")
                || count
                    != event.getInt("reads")
                        + event.getInt("writes")
                        + event.getInt("accepts")
                        + event.getInt("connects")
                || event.getLong("nativeBytes") < 0
                || event.getLong("pollNanos") < 0
                || event.getLong("dispatchNanos") < 0)
              throw new AssertionError("Invalid batch accounting: " + event);
            read += event.getLong("receivedBytes");
            written += event.getLong("writtenBytes");
          }
          case "dev.elide.TransportTlsHandshake" -> {
            if (!event.getString("outcome").equals("success")
                || !event.getString("protocol").equals("h2")
                || !channels.add(event.getString("channelId")))
              throw new AssertionError("Incorrect or duplicate handshake: " + event);
            handshakes++;
          }
          case "dev.elide.TransportCopy" -> {
            if (event.getLong("bytes") <= 0
                || event.getLong("socketId") == 0
                || event.getString("channelId").isEmpty())
              throw new AssertionError("Invalid copy accounting: " + event);
            copies.add(event.getString("reason"));
          }
          case "dev.elide.TransportShutdown" -> {
            if (event.getInt("status") != 0 || event.getLong("retainedBytes") != 0)
              throw new AssertionError("Driver failed to reclaim test buffers: " + event);
            shutdowns++;
          }
          default -> {}
        }
      }
      if (drivers.isEmpty()
          || read < 8
          || written < 8
          || handshakes != 2
          || shutdowns < 2
          || !copies.containsAll(List.of("tcp-write", "tls-write", "tls-read")))
        throw new AssertionError("Missing transport events: " + events);
      for (RecordedEvent event : events) {
        if (event.getEventType().getName().equals("dev.elide.TransportTlsHandshake")
            && !drivers.contains(event.getLong("driverId")))
          throw new AssertionError("Handshake has no correlated batch driver");
      }
      System.out.println("Native transport JFR accounting and disabled-event checks passed");
    } finally {
      Files.deleteIfExists(file);
    }
  }
}
