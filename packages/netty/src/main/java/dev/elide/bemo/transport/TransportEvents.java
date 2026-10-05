/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import jdk.jfr.*;
import org.jspecify.annotations.Nullable;

/**
 * Optional profiling at the managed/native boundary; no endpoint names or payloads are recorded.
 */
public final class TransportEvents {
  private TransportEvents() {}

  @Name("dev.elide.TransportBatch")
  @Label("Native transport completion batch")
  @Category({"Elide", "Transport"})
  @Description("One poll and dispatch pass, including idle passes; opt in for batching analysis")
  @StackTrace(false)
  @Enabled(false)
  public static final class Batch extends Event {
    public long driverId;
    public @Nullable String backend;
    public int capacity;
    public int completions;
    public int activeChannels;
    public int reads;
    public int writes;
    public int accepts;
    public int connects;
    public int errors;

    @Timespan(Timespan.NANOSECONDS)
    public long requestedWaitNanos;

    @Timespan(Timespan.NANOSECONDS)
    public long pollNanos;

    @Timespan(Timespan.NANOSECONDS)
    public long dispatchNanos;

    @DataAmount(DataAmount.BYTES)
    public long receivedBytes;

    @DataAmount(DataAmount.BYTES)
    public long writtenBytes;

    @DataAmount(DataAmount.BYTES)
    public long nativeBytes;
  }

  @Name("dev.elide.TransportTlsHandshake")
  @Label("Native TLS handshake")
  @Category({"Elide", "Transport"})
  @StackTrace(false)
  public static final class TlsHandshake extends Event {
    public long driverId;
    public long socketId;
    public @Nullable String channelId;
    public @Nullable String role;
    public @Nullable String protocol;

    @Description("success, error, or closed")
    public @Nullable String outcome;
  }

  @Name("dev.elide.TransportCopy")
  @Label("Native transport staging copy")
  @Category({"Elide", "Transport"})
  @Description("Managed write staging or Rustls plaintext staging; not all internal Rustls copies")
  @StackTrace(false)
  @Enabled(false)
  public static final class Copy extends Event {
    public long driverId;
    public long socketId;
    public @Nullable String channelId;

    @Description("tcp-write, tls-write, or tls-read")
    public @Nullable String reason;

    @DataAmount(DataAmount.BYTES)
    public long bytes;
  }

  @Name("dev.elide.TransportShutdown")
  @Label("Native driver shutdown")
  @Category({"Elide", "Transport"})
  @StackTrace(false)
  public static final class Shutdown extends Event {
    public long driverId;
    public @Nullable String backend;
    public int busyRetries;
    public int status;

    @Description("Payload storage still retained after driver and completion buffer release")
    @DataAmount(DataAmount.BYTES)
    public long retainedBytes;
  }

  static @Nullable Batch batch(long driver, String backend, int capacity, long timeout) {
    if (!FlightRecorder.isInitialized()) return null;
    Batch event = new Batch();
    if (!event.isEnabled()) return null;
    event.driverId = driver;
    event.backend = backend;
    event.capacity = capacity;
    event.requestedWaitNanos = timeout;
    event.begin();
    return event;
  }

  static @Nullable TlsHandshake handshake(NativeStreamChannel channel) {
    if (!FlightRecorder.isInitialized()) return null;
    TlsHandshake event = new TlsHandshake();
    if (!event.isEnabled()) return null;
    event.driverId = channel.io().driver();
    event.socketId = channel.socket;
    event.channelId = channel.id().asLongText();
    event.role = channel.parent() == null ? "client" : "server";
    event.begin();
    return event;
  }

  static void handshakeDone(
      @Nullable TlsHandshake event, String outcome, @Nullable String protocol) {
    if (event == null) return;
    event.end();
    if (!event.shouldCommit()) return;
    event.outcome = outcome;
    event.protocol = protocol;
    event.commit();
  }

  static void copy(NativeStreamChannel channel, String reason, int bytes) {
    if (!FlightRecorder.isInitialized()) return;
    Copy event = new Copy();
    if (!event.isEnabled()) return;
    event.driverId = channel.io().driver();
    event.socketId = channel.socket;
    event.channelId = channel.id().asLongText();
    event.reason = reason;
    event.bytes = bytes;
    event.commit();
  }

  static @Nullable Shutdown shutdown(long driver, String backend) {
    if (!FlightRecorder.isInitialized()) return null;
    Shutdown event = new Shutdown();
    if (!event.isEnabled()) return null;
    event.driverId = driver;
    event.backend = backend;
    event.begin();
    return event;
  }
}
