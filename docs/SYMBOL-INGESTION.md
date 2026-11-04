# Private final-symbol ingestion

`.github/workflows/on.symbols.yml` observes successful producer workflows and calls
`job.symbols.yml`. The latter can also ingest an earlier successful producer run
without recompiling it. It runs from the repository's linked trusted upload branch,
uses `elide-dev/setup-debuginfo` for a project-scoped 15-minute GitHub credential,
and uploads only final peers selected by the distribution/preservation manifests.

Repository settings supply `DEBUGINFO_DEV_URL`, `DEBUGINFO_DEV_ORG`,
`DEBUGINFO_DEV_ORG_ID`, `DEBUGINFO_DEV_PROJECT` and `DEBUGINFO_DEV_PROJECT_ID`.
`DEBUGINFO_DEV_BRANCH` defaults to `main`; it must match the service repository link.
Private visibility and the exact org/project identifiers are checked before upload.
The private staging destination must be provisioned/verified before enabling uploads.
An unconfigured repository or untrusted upload branch reports its skipped status.
Once configured, failures in trusted ingestion or byte-for-byte readback fail the job.

`.github/symbol-ingestion.json` owns artifact selection and allowed producer refs.
PR artifacts and foreign repository runs are refused; permitted release tags must
resolve to the tested run's exact SHA. Symbols are downloaded and hashed before
identity exchange so compiler/download time does not consume upload credentials.
The canonical helper copies are pinned in `tools/symbol-coverage/INGEST-UPSTREAM.json`.

Receipts bind the original source commit/configuration, final binary/debug hashes,
producer run/artifact records and manifest hashes to server artifact/debug IDs and
fresh SHA-256 readback. The signed server Build records the upload workflow driver;
the actual producer commit is separately retained in receipt and display-archive
metadata, including builds checked out from a tag. Neither field should be confused
with an independently attested original build.

ELF and individual Mach-O DWARF peers use the supported CLI contract. Standalone
PDB and split-DWARF companion uploads remain explicit gaps. Exact obfuscation maps
remain private, hashed build artifacts; the service does not interpret that map
format. Original producer DWARF errors, missing source/debug information, copied
supplier gaps and source-provenance limits remain in the receipts. Successful ingest
and readback do not certify complete source attribution or guest/JIT capture.

See the [shared contract](https://github.com/elide-dev/debuginfo.dev/blob/main/tools/symbol-coverage/README.md#private-ci-ingestion).
