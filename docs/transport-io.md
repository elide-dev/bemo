# I/O ownership and batching

The Netty adapter and Rust driver share responsibility for buffer lifetime.
Netty tracks application references and write progress. Rust tracks native
allocations and memory still accessible to the kernel.

## Ownership rules

| Resource | Who keeps it alive | When it can be reused or released |
| --- | --- | --- |
| Mutable buffer | Its native handle and Java references | After borrowed views are no longer used |
| Frozen buffer | Its handles, retained slices, and pending send leases | After the last reference and kernel lease are released |
| Borrowed direct send storage | The caller for the duration of the syscall | When the inline send returns |
| Pending receive storage | The driver | After completion or cancellation has retired kernel access |
| Reusable send staging | The I/O handler's allocation owner | Between inline sends, or at handler teardown |

A memory view does not own its allocation. Keep the owning buffer alive while
using the view. Freezing a native buffer invalidates earlier mutable access;
those old views cannot be used for a later inline send.

Cancellation is a request to stop work. It does not immediately return memory
to the caller. A pending operation keeps its lease until the driver observes
that kernel access has ended, even if the caller has released its handle.

## Writes

The adapter batches flushed buffers into sends of at most **128 KiB** and
**64 NIO views**. A buffer with more components spans multiple sends. Partial
writes advance both the cached NIO views and Netty's outbound buffers by the
completed byte count.

Unix backends first try a bounded nonblocking send. They can borrow
exclusively owned, unfrozen direct buffers until the syscall returns. Other
buffers use one reusable staging allocation per I/O handler. An inline send
creates no pending operation or send completion.

Backpressure, an occupied write lane, and IOCP use the
asynchronous path. Before falling back, the handler releases reusable staging
so its budget can cover the immutable send allocation. Frozen buffers use their
native handles. Gathered sends retain a lease for each region.

Inline write attempts are limited by Netty's write-spin count. Remaining work
resumes through the event-loop task queue, allowing other channels to run.

## Reads

A receive can finish during submission or remain pending:

- An immediate positive result returns one mutable handle and a view of the
  initialized bytes. There is no outstanding operation or queued completion.
- A pending read retains its storage in the driver until completion or
  cancellation retires kernel access.
- Immediate EOF and errors return no storage.

Receive allocations charge their pinned capacity to the owner budget. Views
expose only the bytes initialized by the kernel. When the default adaptive
allocator grows to at least 16 KiB, Bemo uses a 64 KiB floor to reduce handoffs
between TLS records. Explicit receive allocators keep their requested sizes
and feedback.

On the polling backend, a full receive preserves readiness so another bounded
read can drain the socket. A short or blocked receive waits for fresh readiness.
A batch handles at most **16 messages** before yielding to the event loop.
Read callbacks cannot recursively submit another read while that batch is being
dispatched.

## TLS writes and read scheduling

Transport-owned TLS keeps outgoing ciphertext until every byte of its record
has been accepted. A partial socket write does not complete the application
write. Read submission waits until the TLS engine pump returns, so an inline
receive cannot leave work behind the pump's reentry guard.

The separate native HTTP/TLS path applies the same record-lifetime rule in Rust.
It tries inline ciphertext sends on Unix backends and uses completion-based
sends for backpressure and IOCP. Each drive yields after 128 KiB or its
transition limit and schedules any remaining work.

## Polling and wakeups

A driver reuses its completion vectors between polls. Callback polls and
ordinary polls have separate scratch storage so reentrant callbacks do not
overwrite results being dispatched.

External wakeups coalesce until the next poll. The I/O thread does not wake
itself. It resets the wake flag before checking the event-loop task queue, so
an external producer racing with a blocking poll still signals the driver.

## Shutdown

For a Netty application:

1. Close the listening channel and active connections.
2. Shut down the event-loop groups and wait for termination. The handlers retire
   pending native operations and release their drivers and staging allocations.
3. Close the workload after all event-loop groups using it have stopped.
4. Close application-owned TLS contexts and release retained buffers once their
   users have finished.

The [README setup](../README.md#java-and-netty) shows event-loop and workload
cleanup. The [transport examples](../tests/transport/java) include TLS context
and channel lifetimes. Keep the FFM library loaded while native handles or
views remain in use.

For C and Rust handle callers, a busy release means cancellation has not
finished. Continue driving completion on the owner thread before retrying the
release. Do not free storage still leased to a pending operation.

## Checks

The shared FFM and Native Image contracts cover borrowed-source reuse, handle
release, workload and driver isolation, gathered-send leases, and asynchronous
fallback. Transfer tests cover fragmented reads, more than 64 components,
partial writes, tight budgets, retained slices, and teardown. TLS tests cover
terminal failure and delivery of the queued fatal alert.

See [native safety](native-safety.md) for sanitizers and ownership checks, and
[measurement](measurement.md) for performance tests.
