# Bounded ordinary allocation reuse

Ordinary `Buffer::new` allocations now reuse idle native storage with exactly
the requested capacity, retaining both the payload and its `Arc` descriptor. Receive buffers keep their existing best-fit,
owner-affine policy. Provided receive rings keep their dedicated return slots.
Only the final allocation reference can return storage: frozen slices, mutable
splits, and kernel leases all prevent admission until they retire.

Idle capacity is excluded from live `Budget::used` charges and reported through
`pool_retained`. The thread-local pool remains bounded to 16 entries and 256 KiB.
An ordinary allocation can be retained only within half its releasing owner's
limit; receive admission retains the existing one-eighth limit. Oversized
allocations and storage released after owner closure are freed directly.
Thread teardown frees every idle entry, and trimming prefers closed owners.

An exact ordinary entry may move to another owner after its last lease ends.
Reuse must reserve its full capacity against the requesting owner before
constructing the new allocation; exhausted and closed owners cannot take it.
The old owner's live charge has already ended. No pointer or slice from the old
allocation survives this transfer. Receive entries remain owner-affine and
cannot be used as ordinary entries.

Every reused mutable view starts with zero initialized length. Rust callers
must initialize before reading; ABI allocation still zero-initializes the full
foreign-visible capacity, preventing disclosure of a previous payload. Pooling
reduces allocator traffic but deliberately preserves that zeroing cost.

Contracts cover exact geometry, last-slice lifetime, changed owner accounting,
stale-byte clearing, exhausted/closed admission, bounded retention, cross-thread
release, and thread teardown. The existing buffer and ABI fuzz targets exercise
allocation, initialization, slicing, freezing, and releases.

A local ARM64 macOS Criterion probe (20 samples, 200 ms warmup, 500 ms
measurement) observed warm ordinary 4 KiB allocate/drop near 12 ns versus the
prior 29 ns, and 64 KiB near 12 ns versus 86 ns. These exclude foreign-view
zeroing and ABI registry work. They do not predict end-to-end throughput;
CodSpeed's Linux simulation and transport runs qualify the complete stack.
