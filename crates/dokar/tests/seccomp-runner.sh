#!/usr/bin/env bash
# Cargo target runner: run one test binary in a container under Docker's default seccomp profile,
# which refuses io_uring_setup, or under ELIDE_TRANSPORT_SECCOMP_PROFILE (a profile JSON path).
# The image defaults to the host's distribution so glibc matches.
set -euo pipefail
binary=$(realpath "$1")
shift
image=${ELIDE_TRANSPORT_SECCOMP_IMAGE:-$(. /etc/os-release && echo "$ID:$VERSION_ID")}
profile=()
if [[ -n ${ELIDE_TRANSPORT_SECCOMP_PROFILE:-} ]]; then
  profile=(--security-opt "seccomp=$(realpath "$ELIDE_TRANSPORT_SECCOMP_PROFILE")")
fi
exec docker run --rm --network none "${profile[@]}" \
  -e ELIDE_TRANSPORT_TEST_BACKEND -e ELIDE_TRANSPORT_TEST_EXPECT_FALLBACK -e RUST_BACKTRACE \
  -v "$(dirname "$binary"):/t:ro" -w /t "$image" "/t/$(basename "$binary")" "$@"
