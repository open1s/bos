#!/usr/bin/env bash
# Release-mode extension budgets from docs/bsh-extension-gold-standard.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> release budgets"
cargo test --release -p bsh --lib gold_standard_release -- --ignored --nocapture
