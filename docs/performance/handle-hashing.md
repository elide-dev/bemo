# Numeric registry hash distribution

Buffer registries now mix the high fingerprint bits of their
internally minted `u64` keys with a multiplicative integer hash. Identity hashing
leaves handles in the same domain with common high fingerprint bits, making a
hash table compare more keys after matching a control group. The mixer spreads
those seven bits while preserving low bucket bits and their sequential locality.
Scattering bucket positions would lose the direct placement of sequential keys.

The change preserves full keys, monotonic identity minting, buffer-domain
routing, mutexes, and callback reentry rules. It neither recycles handles nor
replaces the registries with slot tables. Owner, driver, socket, operation,
and waker maps keep their current policies; untrusted string keys are unaffected. There
is no claim of reduced lock contention from single-threaded measurements.

Distribution tests cover sequential keys, strides, and domain prefixes,
including maximum `u64` values and complete-key distinctions. Existing ABI
contracts cover stale/forged handles, owner closure, repeated release, retained
slices, and cross-thread release. The registry benchmark measures owner
lookup, buffer view, allocate/release, and slice/release with 1, 64, and 4096
live entries, compared against the preceding PR.

Local ARM64 macOS probes (30 samples, 300 ms warmup, 1 s measurement) against
PR 4 measured approximately 18% less time for allocate/release and 57% less for
slice/release at 4096 live handles. Direct buffer lookup added approximately
0.2 ns. Slice/release at one live handle added approximately 1.5 ns; at
64 handles it improved approximately 0.5 ns. Allocate/release at 1 and 64 live
handles regressed approximately 24% and 30% (14 and 20 ns), respectively, with
more variation than the lookup and slice probes. The fingerprint work removes
many candidate comparisons in denser tables, but these small-table costs need
CI qualification. Owner lookups showed no benefit in earlier probes, so their
identity-hash policy remains unchanged; this final run also measured a small
1–2% drift in those unchanged lookups.
These local single-threaded probes do not establish end-to-end throughput.

The stack also keeps allocation-lease drop inline, checks shared reference count
before reading pool metadata, and transfers its `Arc` with `ManuallyDrop` instead
of clearing an `Option`. CodSpeed's PR 3 retain/drop profile identified the
option replacement and metadata checks as added work. The transfer occurs once
in `Drop`; the field never drops automatically or is read again. Reference
counting remains in the standard `Arc`, and only a final lease can enter the
pool. Buffer/provided-slot and ABI Miri contracts cover the transfer.

Local shared retain/slice probes measured approximately 3.4 ns against the prior
3.2 ns, retaining a small absolute shared-view cost for allocation reuse. The
PR 3/4 CodSpeed comparisons changed physical CPUs, affecting their simulated
cache models; do not attribute their complete ratios to code alone. The final
stack needs its own CI comparison, including these shared-view workloads.
