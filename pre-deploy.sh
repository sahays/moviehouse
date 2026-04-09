#!/usr/bin/env bash
set -euo pipefail

echo "==> Formatting code..."
cargo fmt --all

echo "==> Linting code..."
cargo clippy --all-targets --all-features -- -D warnings

echo "==> Pre-deploy checks passed."
