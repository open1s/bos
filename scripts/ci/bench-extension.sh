#!/usr/bin/env bash
# Release-mode extension budgets from docs/bsh-extension.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> release budgets (core)"
cargo test --release -p ferris-core --features tokio --lib gold_standard_release -- --ignored --nocapture

echo "==> release budgets (bsh)"
cargo test --release -p bsh --lib gold_standard_release -- --ignored --nocapture
