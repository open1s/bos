#!/usr/bin/env bash
# BSH Extension Framework gold-standard conformance gate.
#
# This is the CI entry point described by docs/bsh-extension.md.
# It proves the composition semantics and the host install path; the
# performance and self-hosting gates are added as they land.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> format"
cargo fmt -p bsh -- --check

echo "==> clippy"
cargo clippy -p bsh --lib --tests -- -D warnings

echo "==> extension suite"
cargo test -p bsh --lib extension::

echo "==> framework runtime conformance"
cargo test -p ferris-runtime --features "wasm extism cordis"

echo "==> SDK conformance"
cargo test -p ferris-sdk

echo "==> composition conformance"
cargo test -p ferris-core --features tokio --lib composable::

echo "==> budget smoke"
cargo test -p ferris-core --features tokio --lib composable::tests::gold_standard

echo "==> all-in-one contribution coverage"
cargo test -p bsh --lib extension::e2e::the_allinone_example -- --nocapture

echo "==> Cordis runtime coverage"
cargo test -p bsh --lib extension::runtime::cordis -- --nocapture
cargo test -p bsh --lib extension::e2e::the_cordis_example -- --nocapture

if [[ "${GOLD_STANDARD_RELEASE:-${CI:-0}}" == "1" || "${GOLD_STANDARD_RELEASE:-${CI:-0}}" == "true" ]]; then
  echo "==> release budgets"
  bash scripts/ci/bench-extension.sh
fi

echo "gold standard: gates currently enforced passed"
