# Release automation and verification

Release Please maintains a release PR independently of verification, following
Bali's workflow split. Its simple version strategy updates `.version`, the
Cargo workspace version, the core dependency constraint, both workspace
entries in `Cargo.lock`, and the `bemo` entry in `fuzz/Cargo.lock`. The fuzz
workspace has its own lockfile; keep its path dependency version in sync on every
release bump. `bemo-fuzz` remains unpublished at `0.0.0`. `make check` validates
both workspaces with `--locked`, so stale locks fail instead of being rewritten.
Elide and generated Maven POMs read `.version` directly.
Release Please selects the next version from Conventional Commits and records
it in the manifest. Snapshot builds cannot publish a release.

On main pushes, the full reusable verification flow runs first. Its hosted Linux
and macOS package jobs test the staged artifacts and attest the exact platform
ZIPs using `actions/attest`. The release job then creates or resumes a draft for
that tested commit, downloads artifacts from the same workflow run, and verifies
their provenance against the build workflow, main ref, source commit, and hosted
runner identity. It never recompiles release assets.

After GitHub publication, two separate jobs publish the tested Maven artifacts.
GitHub Packages verifies and merges the platform bundles from the same run;
Central fetches the immutable GitHub release, verifies its source attestations
and signed checksums, adds PGP signatures, and validates and publishes through
the Portal. Central checks the public POM/JAR bytes before reporting success.
Only the release job's selected version triggers Central; later development
commits with the same version do not republish it. Snapshot main pushes publish
only to GitHub Packages. See [Maven delivery](publishing.md).

The release job outputs the selected version for both a new draft and a completed
immutable release at the exact tested source revision. On retries it skips edits
to published GitHub assets, allowing Central to recover independently. Central's
`deploy` operation verifies an existing public version instead of trying to
replace it. Upload records retain the deployment UUID and per-artifact hashes
for recovery after interrupted validation.

The release environment scopes publication. The job signs every platform ZIP,
provenance bundle, and checksum list with public Sigstore keyless signing. It
verifies each signature before upload, checks uploaded asset digests, then
publishes the draft last. Enabling repository release immutability freezes its
tag and assets at publication. Repository administrators must keep this policy
enabled. Its settings endpoint requires `Administration: read`, which
`GITHUB_TOKEN` cannot hold, so the workflow does not query it. After publication,
it requires the release API's `immutable` field to be explicitly `true`; a missing
or false value fails the job. This is a post-publication check, not a preflight
guarantee if an administrator disables the policy. No admin token is stored in CI.
The workflow refuses publication when any platform/signature is missing or the
tag points elsewhere.
Retry the original failed push run to resume an unpublished draft; a subsequent
commit cannot substitute its artifacts for the original release revision.
Reruns use the original commit's release script, so a script defect requires a
fresh release revision or a separate recovery of the original tested assets.

## Release history

The initial `0.2.0` draft attempt failed because GitHub's release-by-tag endpoint
excluded drafts. The release script now discovers drafts through the paginated
release list and verifies them by release ID. The replacement `v0.2.0` release
was published immutably on October 6, 2026. Its original platform assets remain
the source for Maven Central delivery; newer code belongs in a new version.

## Trust boundary

The target is **SLSA Build Level 2**: version-controlled build instructions,
hosted builds, and authenticated service-generated provenance. This is not a
claim of Level 3 isolation or a reproducible/hermetic build. The dedicated
`linux-amd64-bench` runner measures performance and contributes no release bits.
Published release assets include provenance bundles for consumer verification;
checking in YAML alone is not evidence of a supply-chain level.

Bemo's GitHub repository is public. The release workflow uses GitHub build
attestations and separate public Fulcio/Rekor cosign signatures. Signatures
authenticate the publisher; build attestations identify the build. Release
provenance bundles are attached for verification after Actions artifact
retention expires. Signing identities must use the renamed `elide-dev/bemo`
repository.

## Consumer verification

Download the platform ZIP, corresponding `provenance-<runner>.json`, and the ZIP's
`.sigstore.json` from the same release. Substitute the desired version and its
full tagged commit SHA:

```sh
gh attestation verify bemo-VERSION-linux-x86_64-gnu-unsigned.zip \
  --repo elide-dev/bemo --bundle provenance-ubuntu-24.04.json \
  --signer-workflow elide-dev/bemo/.github/workflows/job.build.yml \
  --source-ref refs/heads/main --source-digest TAGGED_COMMIT_SHA \
  --deny-self-hosted-runners
cosign verify-blob bemo-VERSION-linux-x86_64-gnu-unsigned.zip \
  --bundle bemo-VERSION-linux-x86_64-gnu-unsigned.zip.sigstore.json \
  --certificate-identity https://github.com/elide-dev/bemo/.github/workflows/job.release.yml@refs/heads/main \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

For all assets, verify `SHA256SUMS.sigstore.json` against `SHA256SUMS` with the
same cosign identity, then run `sha256sum -c SHA256SUMS` in the download directory.
These are unsigned Maven repositories *inside* signed release envelopes. Central
uses its own merged repository, PGP signatures, and namespace credentials; see
[publishing](publishing.md). Those credentials are confined to the Central job
in the protected release environment.

Release Please uses `GITHUB_TOKEN`; PRs it creates do not trigger PR workflows
implicitly. The PR job explicitly dispatches the reusable verification flow via
`on.push.yml` on the validated Release Please branch. Dispatch cannot publish a
release: publication requires a main push and successful merged-source checks.
No additional long-lived automation token is required.

References: [GitHub immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases),
[artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations),
and [SLSA Build requirements](https://slsa.dev/spec/v1.2/build-requirements).
