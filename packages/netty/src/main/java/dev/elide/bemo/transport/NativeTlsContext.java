/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import java.nio.charset.StandardCharsets;
import org.jspecify.annotations.Nullable;

/** Immutable Rustls/AWS-LC configuration. Sessions retain it independently of this handle. */
public final class NativeTlsContext implements AutoCloseable {
  final TransportNative api;
  private long handle;

  private NativeTlsContext(TransportNative api, long handle) {
    if (handle == 0) throw new IllegalArgumentException("Invalid native TLS configuration");
    this.api = api;
    this.handle = handle;
  }

  public static NativeTlsContext client(
      TransportNative api, byte[] trustAnchorsPem, String... protocols) {
    return create(api, Workload.id(api), trustAnchorsPem, null, protocols);
  }

  public static NativeTlsContext server(
      TransportNative api, byte[] certificateChainPem, byte[] privateKeyPem, String... protocols) {
    return server(api, Workload.id(api), certificateChainPem, privateKeyPem, protocols);
  }

  public static NativeTlsContext server(
      TransportNative api,
      long workload,
      byte[] certificateChainPem,
      byte[] privateKeyPem,
      String... protocols) {
    return create(api, workload, certificateChainPem, privateKeyPem, protocols);
  }

  private static NativeTlsContext create(
      TransportNative api,
      long workload,
      byte[] certificates,
      byte @Nullable [] key,
      String[] protocols) {
    java.io.ByteArrayOutputStream encoded = new java.io.ByteArrayOutputStream();
    for (String protocol : protocols) {
      byte[] bytes = protocol.getBytes(StandardCharsets.US_ASCII);
      if (bytes.length == 0
          || bytes.length > 255
          || !protocol.equals(new String(bytes, StandardCharsets.US_ASCII)))
        throw new IllegalArgumentException("ALPN identifier must contain 1–255 ASCII bytes");
      encoded.write(bytes.length);
      encoded.writeBytes(bytes);
    }
    long certBuffer = 0, keyBuffer = 0, alpnBuffer = 0;
    try {
      certBuffer = upload(api, workload, certificates);
      if (key != null) keyBuffer = upload(api, workload, key);
      if (encoded.size() != 0) alpnBuffer = upload(api, workload, encoded.toByteArray());
      return new NativeTlsContext(
          api,
          key == null
              ? api.tlsClient(workload, certBuffer, alpnBuffer)
              : api.tlsServer(workload, certBuffer, keyBuffer, alpnBuffer));
    } finally {
      if (certBuffer != 0) api.bufferRelease(certBuffer);
      if (keyBuffer != 0) api.bufferRelease(keyBuffer);
      if (alpnBuffer != 0) api.bufferRelease(alpnBuffer);
    }
  }

  static long upload(TransportNative api, long owner, byte[] bytes) {
    long buffer = api.bufferNew(owner, Math.max(1, bytes.length));
    if (buffer == 0) throw new OutOfMemoryError("Native TLS allocation budget exhausted");
    try {
      api.bufferView(buffer).put(bytes);
      if (api.bufferFreeze(buffer, bytes.length) != 0)
        throw new IllegalStateException("Native TLS input freeze failed");
      return buffer;
    } catch (Throwable error) {
      api.bufferRelease(buffer);
      throw error;
    }
  }

  // Handles are owned by their exact native binding instance.
  @SuppressWarnings("ReferenceEquality")
  synchronized long session(
      TransportNative transport, long workload, long owner, @Nullable String peerName) {
    if (transport != api || handle == 0)
      throw new IllegalStateException("TLS context unavailable for this transport");
    long name =
        peerName == null ? 0 : upload(api, owner, peerName.getBytes(StandardCharsets.UTF_8));
    try {
      long session = api.tlsNew(workload, handle, owner, name);
      if (session == 0)
        throw new IllegalArgumentException("Native TLS session requires a valid peer identity");
      return session;
    } finally {
      if (name != 0) api.bufferRelease(name);
    }
  }

  @Override
  public synchronized void close() {
    if (handle != 0) {
      api.tlsContextRelease(handle);
      handle = 0;
    }
  }

  /**
   * Attach native HTTP with a retained server configuration; a closed context rejects attachment.
   */
  public synchronized int attachHttp(
      long workload, long driver, long socket, long owner, long capacity) {
    return handle == 0 ? -1 : api.socketHttpTls(workload, driver, socket, owner, capacity, handle);
  }
}
