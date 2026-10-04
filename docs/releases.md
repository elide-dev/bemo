# Release automation and verification

Release Please maintains a release PR independently of verification, following
Bali's workflow split. Its simple version strategy updates `.version`, the
Cargo workspace version, the core dependency constraint, and both workspace
entries in `Cargo.lock`. Elide and generated Maven POMs read `.version` directly.
The initial manifest records `0.1.0`; the next release PR selects the next version
from Conventional Commits. Snapshot builds cannot publish a release.

On main pushes, the full reusable verification flow runs first. Its hosted Linux
and macOS package jobs test the staged artifacts and attest the exact platform
ZIPs using `actions/attest`. The release job then creates or resumes a draft for
that tested commit, downloads artifacts from the same workflow run, and verifies
their provenance against the build workflow, main ref, source commit, and hosted
runner identity. It never recompiles release assets.

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

## Trust boundary

The target is **SLSA Build Level 2**: version-controlled build instructions,
hosted builds, and authenticated service-generated provenance. This is not a
claim of Level 3 isolation or a reproducible/hermetic build. The dedicated
`linux-amd64-bench` runner measures performance and contributes no release bits.
An initial successful release and consumer verification are still required to
qualify the deployed workflow; checking in YAML alone is not evidence of a level.

Dokar is currently an internal GitHub repository. GitHub build attestations use
its private Sigstore instance and authenticated repository access. The separate
cosign signatures use public Fulcio/Rekor, making the signing identity and
artifact hashes public. They provide publisher authentication, while build
attestations identify the build. Release provenance bundles are attached for
verification after Actions artifact retention expires.

## Consumer verification

Download the platform ZIP, corresponding `provenance-<runner>.json`, and the ZIP's
`.sigstore.json` from the same release. Substitute the desired version and its
full tagged commit SHA:

```sh
gh attestation verify dokar-VERSION-linux-x86_64-gnu-unsigned.zip \
  --repo elide-dev/dokar --bundle provenance-ubuntu-24.04.json \
  --signer-workflow elide-dev/dokar/.github/workflows/job.build.yml \
  --source-ref refs/heads/main --source-digest TAGGED_COMMIT_SHA \
  --deny-self-hosted-runners
cosign verify-blob dokar-VERSION-linux-x86_64-gnu-unsigned.zip \
  --bundle dokar-VERSION-linux-x86_64-gnu-unsigned.zip.sigstore.json \
  --certificate-identity https://github.com/elide-dev/dokar/.github/workflows/job.release.yml@refs/heads/main \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

For all assets, verify `SHA256SUMS.sigstore.json` against `SHA256SUMS` with the
same cosign identity, then run `sha256sum -c SHA256SUMS` in the download directory.
These are unsigned Maven repositories *inside* signed release envelopes. Central
still needs its own merged repository, PGP signatures, and namespace credentials;
see [publishing](publishing.md). No Central credentials are used by these jobs.

Release Please uses `GITHUB_TOKEN`; PRs it creates do not trigger PR workflows
implicitly. The PR job explicitly dispatches the reusable verification flow via
`on.push.yml` on the validated Release Please branch. Dispatch cannot publish a
release: publication requires a main push and successful merged-source checks.
No additional long-lived automation token is required.

References: [GitHub immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases),
[artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations),
and [SLSA Build requirements](https://slsa.dev/spec/v1.2/build-requirements).
