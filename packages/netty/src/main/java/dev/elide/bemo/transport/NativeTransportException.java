/*
 * Copyright (c) 2024-2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.elide.bemo.transport;

import io.netty.channel.ChannelException;

/** Expected native operation failure; operation context is carried in the message. */
final class NativeTransportException extends ChannelException {
  private static final long serialVersionUID = 1L;
  // Mirrors `OS_ERROR_BASE` in crates/netty-transport/src/abi.rs.
  private static final long OS_ERROR_BASE = 1000;

  NativeTransportException(String message) {
    super(message, null, true);
  }

  private NativeTransportException(String operation, Throwable cause) {
    super(operation + ": " + cause.getMessage(), cause, true);
  }

  static NativeTransportException operation(String operation, long status) {
    String message =
        switch ((int) status) {
          case -4 -> "Connection refused";
          case -5 -> "Connection reset by peer";
          case -6 -> "Connection timed out";
          case -7 -> "Broken pipe";
          case -8 -> "Permission denied";
          case -9 -> "Address already in use";
          case -10 -> "Cannot assign requested address";
          case -11 -> "No route to host";
          case -12 -> "Connection aborted";
          default ->
              status <= -OS_ERROR_BASE
                  ? "Native operation failed (OS error " + (-status - OS_ERROR_BASE) + ")"
                  : "Native operation failed (" + status + ")";
        };
    // A connect that times out is a connect failure, not a generic socket error: the JDK reports
    // it as `ConnectException: Operation timed out` and Netty's own transports as a
    // ConnectTimeoutException, and callers branch on ConnectException. Darwin drops SYNs to a
    // bound-but-unlistening port, so a timeout is the ordinary shape of "nothing accepts here".
    final boolean connecting = "connect".equals(operation);
    Throwable cause =
        switch ((int) status) {
          case -4 -> new ConnectFailure(message);
          case -6 -> connecting ? new ConnectFailure(message) : new SocketFailure(message);
          case -9, -10 -> new BindFailure(message);
          default -> new SocketFailure(message);
        };
    return new NativeTransportException(operation, cause);
  }

  static UnsupportedOperationException unsupported(String message) {
    return new Unsupported(message);
  }

  private static final class Unsupported extends UnsupportedOperationException {
    private static final long serialVersionUID = 1L;

    Unsupported(String message) {
      super(message);
    }

    @Override
    public Throwable fillInStackTrace() {
      return this;
    }
  }

  private static final class ConnectFailure extends java.net.ConnectException {
    private static final long serialVersionUID = 1L;

    ConnectFailure(String message) {
      super(message);
    }

    @Override
    public Throwable fillInStackTrace() {
      return this;
    }
  }

  private static final class BindFailure extends java.net.BindException {
    private static final long serialVersionUID = 1L;

    BindFailure(String message) {
      super(message);
    }

    @Override
    public Throwable fillInStackTrace() {
      return this;
    }
  }

  private static final class SocketFailure extends java.net.SocketException {
    private static final long serialVersionUID = 1L;

    SocketFailure(String message) {
      super(message);
    }

    @Override
    public Throwable fillInStackTrace() {
      return this;
    }
  }

  @Override
  public Throwable fillInStackTrace() {
    return this;
  }
}
