#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$REPO_DIR"

echo "==> Formatting Rust..."
cargo fmt --all

echo "==> Linting Rust..."
cargo clippy --all-targets --all-features -- -D warnings

echo "==> Formatting React..."
(cd frontend && npx prettier --write src)

echo "==> Linting React..."
(cd frontend && npm run lint)

echo "==> Pre-deploy checks passed."
