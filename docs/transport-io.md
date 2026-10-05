# Netty I/O ownership and batching

The Netty adapter batches flushed byte buffers into sends bounded by 128 KiB and
64 NIO views. Partial writes advance both Netty's cached NIO views and the
outbound buffers by the completed byte count. Buffer component counts can exceed
one batch; omitted components belong to a later send.

On the polling backend, the adapter first tries a bounded nonblocking inline
send. This borrows initialized direct storage only until the syscall returns;
it creates no operation or send completion. Exclusively owned, unfrozen direct
buffers can be borrowed directly. Other buffers use one reusable staging
allocation per I/O handler. Staging is charged to the handler's allocation owner
and released during handler teardown.

Backpressure, an occupied write lane, and completion backends use the ordinary
asynchronous path. Before that fallback, reusable staging is released so a tight
owner can afford an immutable send allocation. Kernel leases survive original
handle release and cancellation until native retirement. Frozen native buffers
use their native handles, including gathered sends of independently leased
regions; an old mutable view is never revived for an inline send. Inline write
spins are bounded by Netty's configured write-spin count, then resume through the
event loop's task queue.

Default adaptive receive allocations that have grown to at least 16 KiB use a
64 KiB floor to amortize completion handoff across TLS records. Explicit receive
allocators retain their requested sizes and feedback. Private receive storage
charges its pinned capacity and exposes only bytes initialized by the kernel.

Receive submission returns bytes immediately when CompIO can complete the read
at submission. Pending reads still use the ordinary operation, cancellation,
and retirement path. Immediate positive results publish one mutable handle and
its initialized view, without an operation or queued completion. EOF and errors
publish no storage. The pinned polling fork preserves readiness after a full
receive so a bounded read can drain bytes still queued in the socket. A short
receive waits for fresh level-triggered readiness, avoiding a syscall just to
observe backpressure. Blocked attempts also wait until the kernel reports
readiness again.

Ready reads are bounded to 16 messages before yielding to the event loop. Read
callbacks cannot recursively submit another read while this batch is dispatching.
Transport-owned TLS defers read submission until its engine pump returns, so
ciphertext arriving inline cannot strand work behind the pump's reentry guard.

The driver reuses its completion scratch vectors between polls. Callback and
ordinary polls have separate scratch storage, preserving callback reentry and
retirement rules. External wakeups coalesce until the next poll; the I/O thread
does not wake itself. The wake flag resets before checking the event loop's task
queue, so a producer racing with a blocking poll still signals the driver.

The shared FFM/C-API contract covers borrowed-source reuse and release, workload
and driver isolation, gathered-send lease retention, and asynchronous fallback.
Transfer tests exercise fragmented reads, more than 64 components, partial
writes, tight allocation budgets, retained slices, and teardown. The TLS engine
also tests terminal failure and delivery of its queued fatal alert. Performance
comparisons use the procedures in [measurement.md](measurement.md).
