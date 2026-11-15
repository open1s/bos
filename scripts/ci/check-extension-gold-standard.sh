#!/usr/bin/env bash
# BSH Extension Framework gold-standard conformance gate.
#
# This is the CI entry point described by docs/bsh-extension.md.
# It proves the composition semantics and the host install path; the
# performance and self-hosting gates are added as they land.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# A caller that already ran the workspace gate on this exact tree (the rust
# workflow's Format/Lint/Test steps run fmt --all, clippy --all-targets and the
# whole test suite) does not need the same answers twice. Setting this keeps
# only the feature-gated cases the workspace test cannot reach.
COVERED="${GOLD_STANDARD_COVERED:-0}"

if [[ "$COVERED" != "1" ]]; then
  echo "==> format"
  cargo fmt -p bsh -- --check

  echo "==> clippy"
  cargo clippy -p bsh --lib --tests -- -D warnings

  echo "==> extension suite"
  cargo test -p bsh --lib extension::
fi

echo "==> framework runtime conformance"
# The pure-compute cancellation tests race the epoch interrupt under CI load
# and fail intermittently; one retry keeps the gate meaningful without
# hiding a deterministic failure.
for attempt in 1 2; do
  if cargo test -p ferris-runtime --features "wasm extism cordis"; then
    break
  fi
  if [ "$attempt" -eq 2 ]; then exit 1; fi
  echo "::warning::runtime conformance attempt $attempt failed; retrying"
done

if [[ "$COVERED" != "1" ]]; then
  echo "==> SDK conformance"
  cargo test -p ferris-sdk
fi

echo "==> composition conformance"
cargo test -p ferris-core --features tokio --lib composable::

echo "==> budget smoke"
cargo test -p ferris-core --features tokio --lib composable::tests::gold_standard

if [[ "$COVERED" != "1" ]]; then
  echo "==> all-in-one contribution coverage"
  cargo test -p bsh --lib extension::e2e::the_allinone_example -- --nocapture

  echo "==> Cordis runtime coverage"
  cargo test -p bsh --lib extension::runtime::cordis -- --nocapture
  cargo test -p bsh --lib extension::e2e::the_cordis_example -- --nocapture
fi

if [[ "${GOLD_STANDARD_RELEASE:-0}" == "1" || "${GOLD_STANDARD_RELEASE:-0}" == "true" ]]; then
  echo "==> release budgets"
  bash scripts/ci/bench-extension.sh
fi

echo "gold standard: gates currently enforced passed"
