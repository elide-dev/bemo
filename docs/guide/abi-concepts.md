# ABI concepts

The [C ABI reference](../reference/c-abi.md) lists every function. This page
explains the rules they share. The Java bindings follow the same rules.

## Versions and capabilities

Call `bemo_abi_version()` before resolving anything else, then
`bemo_capabilities()`. Capabilities are additive bits, and unknown bits must be
ignored:

| Bit | Name | Meaning |
| --- | --- | --- |
| 1 | `BEMO_CAP_TRANSPORT_V3` | The complete Elide transport ABI 3 (`elide_transport_*`) |
| 2 | `BEMO_CAP_NATIVE_ALLOC` | `bemo_alloc`, `bemo_free`, and `bemo_alloc_live` |

A capability is advertised only once its operations and their ownership tests
exist.

## Handles

Resources (owners, buffers, drivers, sockets, TLS contexts and sessions, HTTP
exchanges) are opaque 64-bit handles. Zero is invalid, handles are never reused,
and operations on a released handle fail instead of touching freed memory.

A handle owns a reference. A pointer or `ByteBuffer` view only borrows memory:
abandon writable views before freezing a buffer, and all views before releasing
it.

## Buffers and leases

A mutable buffer exposes writable storage. Freezing it publishes its initialized
bytes as immutable storage that can be sliced or retained by a send. A pending
kernel operation holds its own lease, independent of your handle, so releasing
a handle or cancelling an operation never ends kernel access early.
Cancellation is not completion.

## Threads

Driver operations, and the sockets registered with a driver, belong to the
driver's owner thread. Any thread may wake a driver, and buffers and
allocations may be released from any thread. Callbacks run on the owner thread
and may respond, close, or release; Bemo releases its internal borrows before
invoking them.

## Workloads

Every call that starts work names a workload: an owner handle from
`elide_transport_owner_new`. Unknown, released, or closed workloads start
nothing. Work on an existing socket must name the workload that created it, and
accepted sockets inherit their listener's workload. Closing a workload cancels
its operations and leaves other workloads, even on the same driver, untouched.

## Errors

Negative status codes report failure. Synchronous failures set a per-thread
last error that `elide_transport_last_error()` reads and clears. Connect and
listen report portable codes, from -4 (refused) to -12 (aborted), before OS
errors. Driver creation and serving calls report an OS error as
`-(1000 + errno)`: for example, a seccomp-refused io_uring setup is -1001 and a
pre-6.1 kernel rejecting a flag is -1022.

## Memory accounting

Native buffers charge their capacity to an owner's budget, including retained
slices, and allocation fails when the budget is exhausted. `bemo_alloc` memory is
deliberately unbudgeted and reported process-wide by `bemo_alloc_live`.
