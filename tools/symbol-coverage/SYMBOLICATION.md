# Remote address qualification

`symbolicate.py` qualifies **reviewed sample addresses**, not all frames in a
distribution. It reports availability, exact pairing, source/inline attribution,
producer verification and untested variable quality separately. A useful lookup
never changes `producer-errors` into a verifier pass. A dirty or uninspected build
cannot receive an overall source-quality pass.
`sourceCorrectness` and `inlineCorrectness` are separate: a single-frame source
probe leaves inline correctness `not-tested`. `sourceInline` is the strict
aggregate for the reviewed PCs; no status certifies every function in a module.

## Source expectations and final addresses

Keep reviewed probes in the consumer's `.github/symbolication.json`. Each probe
names a final code symbol and optional instruction offset, then an entire expected
inline chain. Functions, source paths and unique source-statement anchors are
independent inputs. `make-symbolication-case.py` reads source from the manifest's
exact Git revision, obtains the PC from the **final retained symbol table**, and
binds the case to the distributed module, manifest and debug-file SHA-256s. It
does not use a debugger's answers to generate expectations. Dependency source
requires the producer's exact gitlink and a separately supplied source repository.
When a case binds an external preservation record, supply its exact file with
`--preservation-record`. This supports distributions whose manifest preceded the
separate preservation action. Its module/debug hashes must match, its content
must agree with any embedded retention, and a recorded nonzero verifier exit
cannot be relabeled as a pass.

For Runtime's owned C JNI adapter, `regenerate-sqlite-source.py` follows the
Runtime native-toolchain version/checksum, the toolchain tag's exact source
commit, and its SQLite/JNI source pins. It independently regenerates `sqlite3.c`.
The reviewed source anchor supplies the expected function and generated line;
`--source-snapshot-root` supplies that content and its hashed provenance. The
downloaded peer's owning CU must contain the same compiler source MD5. A local
Runtime copy of the adapter cannot substitute for this compiled-source chain.
Metadata-query diagnostics are retained separately and never waive verifier
failures. Changed generation recipes or missing source digests fail for review.
For a repaired preview toolchain with no release tag, `--toolchain-manifest`
accepts its existing coverage manifest only when its repository, exact source
revision, version and native archive SHA match Runtime's configured pin. A new
release is not needed to regenerate and requalify those sources.

Review instruction offsets after compiler/optimizer changes. A moved instruction
or changed inline decision must fail for review; never regenerate expected chains
by recording whatever the symbolizer returns. Normalized PCs must fall inside an
executable `PT_LOAD` range in the shipped ELF. `runtimePc - loadBias` is the object
PC; a synthetic bias is not evidence of a running process. An optional reviewed
no-argument uint32 C export can additionally qualify a real GDB breakpoint,
observed ASLR relocation, source location and return value in a loaded library.

## Automatic developer GDB

Use the documented `dbginfo connect` computer-consent flow for the intended private
project, or an already authorized project-only QUERY credential. No administrator
credential is needed by this gate. Keep a supplied QUERY credential in an
owner-readable file and pass its **path**, never its value in a command or URL:

```sh
python3 tools/symbol-coverage/symbolicate.py \
  --case reviewed-case.json --module shipped-module --manifest symbol-coverage.json \
  --source-root /path/to/repository --query-key-file /private/query.key \
  --client /path/to/dbginfo --symbolizer /path/to/llvm-symbolizer \
  --server https://api.staging.debuginfo.dev --org elide --out result.json
```

Every execution creates a new HOME, config, cache and work directory. Only the
stripped shipped module enters it. No source tree, debug peer, DWO, debug directory
or build-ID link is copied there. `dbginfo setup` provisions the normal integration;
a fresh interactive Bash reads its startup file to obtain the private server.
GDB performs the positive download and must load the exact
peer from its newly created debuginfod cache. The gate checks the downloaded SHA,
identity, architecture and GNU debuglink CRC before any source assertions. It
records client versions and executable hashes and sanitizes credential-bearing
URLs from diagnostics.

`--mode debuginfod-find` uses the ordinary elfutils client with the same setup
and a new empty cache, then LLVM address symbolization. It also accepts shipped
ELF64 for another architecture and thin Mach-O64, checking their executable load
ranges and build ID/UUID. This is explicit remote retrieval and address lookup;
it does not qualify native LLDB automatic loading or execution on that platform.

The no-credential debugger runs in a separate empty cache and must have no source
lines. This headerless client uses the global public index, where private-only
identities are absent. Direct controls name `--org` explicitly and require 401
for absent/invalid credentials, 403 for a known
foreign private-project identity, and 404 for a truly absent identity. A 503 is
reported as a failure/pending dependency, never substituted with 404. Control
requests do not populate the positive debugger's cache. Pick a foreign identity
whose existence in its other private project is independently established.

## CI ordering and scope

The consumer's separate `on.symbolication.yml` runs after successful private
ingestion. It accepts only successful same-repository main-branch ingestion runs
and the receipt artifact for their current run attempt. The existing artifact
validator rechecks producer provenance, exact final hashes and retained peers.
PR artifacts are rejected. No uploads or repository-link changes occur.

Preparation happens before authentication. Each sample then receives fresh access
through the existing pinned `elide-dev/debuginfo-client` action. CI uses
`--mode manual-http`: the supported streaming debuginfod GET, followed by explicit
GDB/LLVM loading. This is **manual remote address symbolization**, not automatic
developer lookup. It avoids relying on an unreleased CLI large-transfer repair.
`--mode manual-cli` exercises `dbginfo get` when the installed client is suitable.

Each result binds its reviewed case SHA, source/module/debug identities and a
new execution nonce. The final step requires every prepared case and rejects old,
missing, mismatched or incomplete success files. Individual cases continue only
to collect evidence; the final step propagates any integration or quality failure.
Results record sampled and unsampled uploaded peers. Linux sample workflows do
not certify macOS, Windows, all supplier modules, PDB/DWO support, Java inlines,
map deobfuscation, JIT frames, variables or source-text publication.

## Repaired producers and reruns

Rebuild after producer repair, preserving final post-BOLT bytes and the actual
source/build metadata. Re-ingest and obtain complete readback receipts first.
Generate new hash-bound cases from the same reviewed probes, then rerun these
gates with fresh caches. Preserve previous failures as separate build evidence.
Changed bytes under an old identity must receive the service's 409 response;
never overwrite them or reuse the earlier build's success. Correct the producer
identity in the producer and retain the 409 as failed collision evidence.

Unmerged consumer workflows have not run under their final main-only OIDC
bindings. Local regression success does not qualify their remote integrations.

## Regression tests

The focused suite checks exact inline chains and source lines, invalid/stale
evidence, archive destinations, private controls, cache isolation, altered debug
bytes with the same identity and real GDB/ASLR operation against a local test
server. That test server is explicitly not real-product remote evidence.

```sh
DEBUGINFO_REQUIRE_SYMBOLICATION_GDB=1 \
DBGINFO_TEST_CLIENT=/path/to/dbginfo \
DBGINFO_TEST_SYMBOLIZER=/path/to/llvm-symbolizer \
python3 -m unittest discover -s tools/symbol-coverage -p 'test_symbolicate*.py' -v
```

The service repository's Live CI group runs this suite with required debugger
tools. Missing tools fail that lane; ordinary artifact-only unit runs may skip
the native debugger fixture when its explicit tool paths are absent.
