/* Copyright (c) 2024-2026 Elide Technologies, Inc. SPDX-License-Identifier: Apache-2.0 */
#ifndef ELIDE_TRANSPORT_H
#define ELIDE_TRANSPORT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ELIDE_TRANSPORT_ABI_VERSION 3

#if UINTPTR_MAX != UINT64_MAX
#error "The transport ABI requires a 64-bit target"
#endif

/* Opaque non-reused handles; zero is invalid. Negative status indicates failure.
 * ABI 3: every entry that starts work takes a leading `uint64_t workload`, an owner handle from
 * owner_new. Unknown, released or closed workloads start nothing, and work on an existing socket
 * must name the workload that created it; accepted sockets inherit their listener's workload. */
typedef struct {
  void *address;
  uint64_t capacity;
  uint64_t length;
  uint64_t flags;
} elide_transport_buffer_view_t;

uint32_t elide_transport_abi_version(void);
/* Last synchronous failure on this thread; reading clears it. listen/connect report portable codes (-4 refused ..
 * -12 aborted) before OS errors. driver_new and serving calls report an OS error as -(1000 + errno), including a
 * refused io_uring setup (seccomp EPERM is -1001, pre-6.1 EINVAL is -1022), else a portable code. */
int32_t elide_transport_last_error(void);
uint64_t elide_transport_owner_new(uint64_t limit);
uint64_t elide_transport_owner_used(uint64_t owner);
/* Release closes the owner's workload (see workload_close) and forgets the handle. */
int32_t elide_transport_owner_release(uint64_t owner);
/* Reject the workload's allocations and new work, and cancel its operations; other workloads on the
 * same drivers are untouched. The calling thread's drivers sweep now, other drivers at their next
 * poll. HTTP-mode sockets close (event kind 6); other sockets keep their handle until closed. */
int32_t elide_transport_workload_close(uint64_t workload);
uint64_t elide_transport_buffer_new(uint64_t owner, uint64_t capacity);
/* Views borrow storage. Abandon writable views before freeze, and all views before release.
 * All views must be quiescent before receive submission, TLS feed/step, or polling into that buffer.
 * A submitted receive buffer is inaccessible until completion; cancellation is not completion. */
int32_t elide_transport_buffer_view(uint64_t buffer, elide_transport_buffer_view_t *output);
int32_t elide_transport_buffer_freeze(uint64_t buffer, uint64_t length);
uint64_t elide_transport_buffer_slice(uint64_t buffer, uint64_t offset, uint64_t length);
int32_t elide_transport_buffer_release(uint64_t buffer);
/* Owner-thread reusable application gzip (zlib-rs), level 0..9, input <=16 MiB.
 * Backend state and bounded scratch are internal overhead; independent frozen output
 * charges workload and survives compressor/input release. Release it with buffer_release.
 * Compression rejects closed workloads and mutable input. Wrong-thread calls reject. */
uint64_t elide_transport_gzip_new(uint64_t workload, uint32_t level);
uint64_t elide_transport_gzip_compress(uint64_t workload, uint64_t encoder, uint64_t input);
int32_t elide_transport_gzip_release(uint64_t encoder);
/* Backend: 0 auto, 1 polling, 2 io_uring, 3 IOCP. All driver calls except wake are owner-thread only. */
uint64_t elide_transport_driver_new(uint64_t workload, uint32_t backend, uint32_t limit);
int32_t elide_transport_driver_backend(uint64_t driver);
int32_t elide_transport_driver_wake(uint64_t driver);
int32_t elide_transport_driver_release(uint64_t driver);
/* Additive (driver diagnostics): AUTO falls back to polling when io_uring setup fails. Writes that error as UTF-8,
 * truncated to the capacity of mutable buffer `output`, and returns its length; 0 when the requested backend runs. */
int32_t elide_transport_driver_fallback(uint64_t driver, uint64_t output);

/* Serving: thread-confined native reactors assigned to independent guest contexts. Startup and listener closure
 * coordinate in native code; live sockets never migrate. Multi-shard listeners require kernel
 * reuse-port distribution. Driver operations are confined to the driver's thread. */
uint64_t elide_transport_serving_new(uint32_t contexts);
uint64_t elide_transport_serving_split_new(uint32_t contexts);
int32_t elide_transport_serving_workers(uint64_t application, uint32_t context);
uint64_t elide_transport_serving_worker_driver(uint64_t workload, uint64_t application, uint32_t context, uint32_t worker, uint32_t backend, uint32_t limit);
/* Positive available physical core count (portable fallback: one shard), <=0 on error. */
int32_t elide_transport_serving_available_cores(void);
/* Guest-thread calls: select the second allowed sibling (or sole CPU), then restore original mask.
 * Start the transport thread BEFORE context_enter so it inherits the full allowed mask. */
int32_t elide_transport_serving_context_enter(uint64_t application, uint32_t replica);
int32_t elide_transport_serving_context_leave(void);
/* Prepare on parent, consume on child: restore that parent's mask before serving pinning.
 * Zero means no action. Tokens are trusted opaque references: consume or release each exactly once.
 * Prepare/release only use TLS and atomics; start is interruptible and reclaims retired masks. */
uint64_t elide_transport_serving_helper_prepare(void);
int32_t elide_transport_serving_helper_start(uint64_t token);
int32_t elide_transport_serving_helper_release(uint64_t token);
/* One owner OS thread per driver. Poll returns before guest dispatch; accepted sockets are already
 * attached locally. Kind 11 closes a local listener (socket); kind 12 reports application readiness
 * (value 1) or closure (value 2), including applications with no listeners. */
/* affinity bit 0 pins the owner; bit 1 hands accepted sockets to separate transport workers. */
uint64_t elide_transport_serving_driver(uint64_t workload, uint64_t application, uint32_t replica, uint32_t backend, uint32_t limit, uint32_t affinity);
uint64_t elide_transport_serving_listen(uint64_t workload, uint64_t driver, uint64_t endpoint, uint64_t signature, int32_t backlog);
int32_t elide_transport_serving_ready(uint64_t driver);
int32_t elide_transport_serving_cpu(uint64_t driver);
int32_t elide_transport_serving_listener_close(uint64_t driver, uint64_t listener);
int32_t elide_transport_serving_close(uint64_t application);

/* Endpoint buffer: 16 address bytes, native-endian u16 port, u16 family (4/6), u32 scope. */
/* Connect also accepts family 1 on Unix: zeroed 24-byte header except family, then nonempty path bytes (no NUL). */
/* Event buffer: u64 operation, socket, value; i64 result; u32 kind, reserved (40 bytes). */
uint64_t elide_transport_socket_listen(uint64_t workload, uint64_t driver, uint64_t endpoint, int32_t backlog, uint32_t reuse);
uint64_t elide_transport_socket_connect(uint64_t workload, uint64_t driver, uint64_t endpoint);
uint64_t elide_transport_socket_accept(uint64_t workload, uint64_t driver, uint64_t listener);
/* A workload other than the listener's leaves the socket with the caller. */
int32_t elide_transport_socket_adopt(uint64_t workload, uint64_t driver, uint64_t socket);
int32_t elide_transport_socket_discard(uint64_t socket);
int32_t elide_transport_socket_address(uint64_t driver, uint64_t socket, uint32_t peer, uint64_t output);
uint64_t elide_transport_socket_receive(uint64_t workload, uint64_t driver, uint64_t socket, uint64_t buffer);
/* Allocate private receive storage charged to owner. Positive completion publishes a new mutable buffer whose
 * capacity equals received length; EOF/errors publish no buffer. Admission failure returns zero
 * without retaining any allocation charge. Completion/retained buffers keep the full charge. */
uint64_t elide_transport_socket_receive_new(uint64_t workload, uint64_t driver, uint64_t socket, uint64_t owner, uint64_t capacity);
/* Pending: status 0 and operation!=0. Immediate: status 1 and operation=0; positive
 * result owns buffer/address, EOF/error owns none. Admission failure: negative status.
 * No completion is queued for immediate results. The address exposes only result bytes;
 * stop mutable access before freezing/submitting/releasing buffer. */
typedef struct {
  uint64_t operation;
  uint64_t buffer;
  void *address;
  int64_t result;
} elide_transport_receive_result_t;
int32_t elide_transport_socket_receive_new_result(uint64_t workload, uint64_t driver, uint64_t socket, uint64_t owner, uint64_t capacity, elide_transport_receive_result_t *output);
uint64_t elide_transport_socket_send(uint64_t workload, uint64_t driver, uint64_t socket, uint64_t buffer, uint64_t offset, uint64_t length);
/* One nonblocking Unix send of 1..131072 borrowed bytes. Returns bytes sent,
 * 0 for backpressure/unsupported backend, or a negative transport error. No operation or
 * completion is created; source is not retained. Serialize source writes with this call. */
int64_t elide_transport_socket_send_inline(uint64_t workload, uint64_t driver, uint64_t socket, const uint8_t *source, uint64_t length);
/* Borrow 1..64 native-endian (address, nonzero length) pairs, totalling at most 131072 bytes.
 * Returns bytes sent, 0 for fallback, or a negative error. No address is retained after return.
 * All ranges must stay readable, alive, and free of concurrent writes until return. */
int64_t elide_transport_socket_send_inline_vectored(uint64_t workload, uint64_t driver, uint64_t socket, const uint64_t *regions, uint32_t count);
/* Additive gathered send: 1..64 native-endian triples (frozen handle, offset, nonzero length).
 * Descriptors are borrowed only during this call; storage is leased until native completion.
 * Release of the original handles does not retire outstanding kernel leases. Returns 0 on rejection. */
uint64_t elide_transport_socket_send_gathered(uint64_t workload, uint64_t driver, uint64_t socket, const uint64_t *regions, uint32_t count);
int32_t elide_transport_socket_close(uint64_t driver, uint64_t socket);
/* UINT64_MAX waits indefinitely, until I/O or an explicit wake. */
int32_t elide_transport_driver_poll(uint64_t driver, uint64_t timeoutNanos, uint64_t batch, uint32_t maximum);
/* Owner-thread upcall: event borrows the ordinary 40-byte completion layout until return.
 * Nonzero stops dispatch, retaining remaining events. Response/close reentry is allowed;
 * recursive polling and a closed callback workload are rejected. No registry borrow or lock spans
 * the callback. */
typedef int32_t (*elide_transport_event_callback_t)(void *context, const void *event);
int32_t elide_transport_driver_poll_callback(uint64_t workload, uint64_t driver, uint64_t timeoutNanos, uint32_t maximum,
                                           elide_transport_event_callback_t callback, void *context);
/* Bounded same-socket request batch. Return the consumed count, negated to stop; zero or partial
 * consumption also stops. Stop after releasing the driver. Unconsumed events remain queued. */
typedef int32_t (*elide_transport_event_batch_callback_t)(void *context, const void *events, uint32_t count);
int32_t elide_transport_driver_poll_batch_callback(uint64_t workload, uint64_t driver, uint64_t timeoutNanos, uint32_t maximum,
                                                 elide_transport_event_batch_callback_t callback, void *context);


int32_t elide_transport_socket_option(uint64_t driver, uint64_t socket, uint32_t option, int32_t value);
int32_t elide_transport_socket_shutdown(uint64_t driver, uint64_t socket, uint32_t direction);

/* HTTP mode: the driver parses requests (event kind 5, value = exchange) and encodes responses. */
int32_t elide_transport_socket_http(uint64_t workload, uint64_t driver, uint64_t socket, uint64_t owner, uint64_t capacity);
/* Clone a server TLS context and activate native HTTP. ALPN accepts h2 and http/1.1. */
int32_t elide_transport_socket_http_tls(uint64_t workload, uint64_t driver, uint64_t socket, uint64_t owner, uint64_t capacity, uint64_t context);
int32_t elide_transport_http_method(uint64_t exchange);
int32_t elide_transport_http_version(uint64_t exchange);
int32_t elide_transport_http_header_count(uint64_t exchange);
int32_t elide_transport_http_keep_alive(uint64_t exchange);
/* kind: 0 method, 1 path, 2 header name, 3 header value, 4 body, 5 whole head; output receives address, length. */
int32_t elide_transport_http_view(uint64_t exchange, uint32_t kind, uint32_t index, uint64_t *output);
/* headers: count records of {name address, name length, value address, value length}. flags bit 1
 * (RESPOND_STREAM) sends the head only and ignores body: bodyLength then declares content-length,
 * or UINT64_MAX for chunked. Parts follow through http_chunk_send. */
int32_t elide_transport_http_respond(uint64_t driver, uint64_t exchange, uint32_t status, const uint64_t *headers, uint32_t count, const uint8_t *body, uint64_t bodyLength, uint32_t flags);
/* u32 pairs within the head: method, path, then name/value per header; returns slots needed. */
int32_t elide_transport_http_spans(uint64_t exchange, uint32_t *output, uint32_t capacity);
int32_t elide_transport_http_release(uint64_t driver, uint64_t exchange);
/* The exchange handle is the address of a 32-byte layout: head address u64, head length u64, span
 * table address u64, span count u32, method u8, version u8, keep-alive u8, reserved u8. It stays
 * readable until http_free, which every request event must reach exactly once (driver thread). */
int32_t elide_transport_http_free(uint64_t driver, uint64_t exchange);
/* Stop admitting request heads, finish accepted bodies/responses, and close drained sockets.
 * Returns the remaining HTTP connection count, or INVALID. Driver thread only. */
int32_t elide_transport_http_drain(uint64_t driver);
/* Retire sockets, keeping delivered body segments alive independently of the driver. */
int32_t elide_transport_http_retire(uint64_t driver);
/* Release retained segment storage on any thread after retirement. */
int32_t elide_transport_http_segment_release_retired(uint64_t segment);
/* Copy a live head into a caller-owned blob (16-byte header: span count u32, head length u32,
 * method, version, keep-alive, padding; then the u32 span table; then head bytes). Any thread. */
uint64_t elide_transport_http_retain(uint64_t exchange, uint64_t *length);
int32_t elide_transport_http_head_release(uint64_t head);
/* Allocate an exchange's response buffer (any thread, once); returns its address or zero. */
uint64_t elide_transport_http_prepare(uint64_t exchange, uint64_t capacity);
/* Send the first `length` bytes of the prepared buffer as the whole response. flags: 1 = close. */
int32_t elide_transport_http_send(uint64_t driver, uint64_t exchange, uint64_t length, uint32_t flags);

/* Body streaming: kind 7 (EVENT_BODY, operation = segment handle, value = exchange, result =
 * length); kind 8 (EVENT_BODY_END, value = exchange, result 0 clean or negative on abort/malformed);
 * kind 9 (EVENT_PART_SENT, value = exchange, result = wire bytes for HTTP/1, header wire bytes
 * then DATA payload credit for HTTP/2); kind 10 (EVENT_RESET, value = exchange, negative result)
 * aborts one HTTP/2 stream without closing siblings. All part credits follow physical writes. A segment
 * handle is the address of a 16-byte layout {const uint8_t *data; uint64_t len;}, read in place;
 * it stays valid until released. ExchangeLayout byte 31 is a flags byte: bit 0 set means a body
 * follows.
 *
 * Two calls, both driver-thread only, retire a segment. `ack` stops charging it against the
 * socket's receive window (the transport's read-ahead) while leaving its bytes valid and in
 * place: the consumer calls it once the bytes have become its own memory, under its own cap.
 * `release` frees the segment; its bytes are invalid afterwards. Release implies ack, so a
 * consumer that only releases behaves exactly as before. Ack is idempotent; release is once per
 * event. A segment that is merely queued, not handed on, must not be acked or the window stops
 * throttling. */
int32_t elide_transport_http_segment_ack(uint64_t driver, uint64_t segment);
int32_t elide_transport_http_segment_release(uint64_t driver, uint64_t segment);
/* Allocate storage for one response part of a streaming exchange: `capacity` payload bytes the
 * caller fills at `*address`, framed in place by chunk_send. Any thread, while the exchange is
 * unfreed; several parts may be prepared ahead. Returns the buffer handle, which the caller owns
 * until chunk_send takes it, or zero on failure. */
uint64_t elide_transport_http_chunk_prepare(uint64_t exchange, uint64_t capacity, uint64_t *address);
/* Queue the first `length` bytes of a prepared part; flags bit 1 (CHUNK_FINAL) ends the response.
 * Driver thread only. Returns 0 (queued, buffer taken), -1 INVALID (buffer stays with caller,
 * unknown handle/exchange or a length beyond the prepared capacity), -2 BUSY (response window
 * full, buffer stays with caller; retry after EVENT_PART_SENT), or a negative portable error (e.g.
 * -3) when the payload diverges from a declared content-length: the buffer is consumed and the
 * connection fails. With flags bit 2 (CHUNK_RETAIN), buffer is frozen and payload starts at
 * offset zero. The caller keeps its handle on EVERY result; success retains a separate lease
 * through send retirement. HTTP/1 chunked framing rejects this flag; length/close framing and
 * HTTP/2 accept it. Mutable handles and lengths beyond initialized bytes are rejected. */
int32_t elide_transport_http_chunk_send(uint64_t driver, uint64_t exchange, uint64_t buffer, uint64_t length, uint32_t flags);

/* Contexts are bound to their workload; session creation and HTTP TLS activation require that same open workload. */
uint64_t elide_transport_tls_client(uint64_t workload, uint64_t roots, uint64_t alpn);
uint64_t elide_transport_tls_server(uint64_t workload, uint64_t chain, uint64_t key, uint64_t alpn);
int32_t elide_transport_tls_context_release(uint64_t context);
/* Once the session's workload closes, feed and write steps fail; progress and close-notify remain. */
uint64_t elide_transport_tls_new(uint64_t workload, uint64_t context, uint64_t owner, uint64_t name);
int32_t elide_transport_tls_feed(uint64_t session, uint64_t input, uint64_t length);
int32_t elide_transport_tls_step(uint64_t session, uint32_t action, uint64_t plaintext, uint64_t offset, uint64_t length, uint64_t output);
int32_t elide_transport_tls_protocol(uint64_t session, uint64_t output);
int32_t elide_transport_tls_release(uint64_t session);

/* ---- SSLEngine (begin) ---------------------------------------------------------------------------
 * Rustls engines driven with javax.net.ssl.SSLEngine semantics over caller memory. Contexts are
 * immutable within one workload; engines may be called from any thread, one call at a time.
 * Context and engine creation require the same open workload. Closing it rejects wrap/unwrap and
 * handshake admission; diagnostics, close controls and release remain available for cleanup.
 * Context flags: bit 0 server (chain + key; otherwise client trust anchors), bit 1 DER input
 * (concatenated certificates, PKCS#8/PKCS#1/SEC1 key) instead of PEM, bit 2 explicitly disables
 * client certificate-chain and hostname verification (handshake signatures remain verified).
 * Bit 2 is invalid for servers. ALPN uses wire format unless bit 3 (policy prefix) is set:
 * u8 version (1), u8 protocol mask (1 TLS 1.2, 2 TLS 1.3; nonzero), u8 cipher count,
 * count big-endian u16 IANA cipher IDs in preference order, then the wire-format ALPN list.
 * Zero cipher count preserves provider defaults. Unknown/duplicate ciphers and invalid policies fail.
 * Bit 4 supplies a client identity: key contains a PEM certificate chain and private key, independent
 * of bit 1. Bit 5 instead supplies selectable identities in key: u8 version=1, u16 identity count
 * (1..64), then per identity u32 chain byte length + concatenated DER certificates, u32 key byte
 * length + DER private key, u16 issuer count (1..64), then u16 name length + DER X.500 issuer name
 * per issuer. All integers are big endian. Issuers must describe the supplied chain's issuer names.
 * The first identity matching an offered issuer (or no issuer hints) and signature scheme is selected.
 * Identity contexts require fresh client authentication; session resumption is disabled.
 * Client trust anchors remain in certificates. Bits 4/5 are mutually exclusive and invalid for servers;
 * without either flag clients pass an empty key. Malformed inputs/key mismatches reject the context. */
uint64_t elide_transport_engine_context_new(uint64_t workload, uint32_t flags, const uint8_t *certificates,
                                            uint64_t certificatesLength, const uint8_t *key, uint64_t keyLength,
                                            const uint8_t *alpn, uint64_t alpnLength);
int32_t elide_transport_engine_context_release(uint64_t context);
/* Clients pass a UTF-8 DNS name or IP literal used for SNI and verification; servers pass none. */
uint64_t elide_transport_engine_new(uint64_t workload, uint64_t context, const uint8_t *name, uint64_t nameLength);
/* Results: -1 invalid, -3 TLS failure (info kind 5 describes it), else bits 0-23 consumed, 24-47 produced,
 * 48-49 SSLEngineResult.Status ordinal, 50-52 HandshakeStatus ordinal, 53 inbound done, 54 outbound done,
 * 55 handshaking, 56 (close inbound) peer close_notify missing. Lengths above 2^24-1 are clamped.
 * Unwrap processes at most one record; without flag bit 0 (shared) it may decrypt the source in place. */
int64_t elide_transport_engine_wrap(uint64_t engine, const uint8_t *source, uint64_t sourceLength,
                                    uint8_t *destination, uint64_t destinationLength);
int64_t elide_transport_engine_unwrap(uint64_t engine, uint8_t *source, uint64_t sourceLength,
                                      uint8_t *destination, uint64_t destinationLength, uint32_t flags);
/* Operations: 0 state, 1 begin handshake, 2 close outbound, 3 close inbound. */
int64_t elide_transport_engine_control(uint64_t engine, uint32_t operation);
/* Kinds: 0 ALPN, 1 protocol code, 2 cipher suite code, 3 peer certificate count, 4 peer certificate DER at
 * index, 5 failure text, 6 32-byte session id, 7 selected client certificate count, 8 selected client
 * certificate DER at index. Byte kinds copy up to capacity and return the full length. */
int32_t elide_transport_engine_info(uint64_t engine, uint32_t kind, uint32_t index, uint8_t *output, uint64_t capacity);
int32_t elide_transport_engine_release(uint64_t engine);
/* ---- SSLEngine (end) ----------------------------------------------------------------------------- */

#ifdef __cplusplus
}
#endif
#endif
