#!/usr/bin/env bash
# Pre-deploy gate. Strict verify-only: nothing is auto-fixed. Run your
# formatters during development; this only confirms the tree is already clean.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_DIR"

echo "Running pre-deploy checks (strict)..."

# ── 1. Rust format — verify only ────────────────────────────────
echo "  Checking Rust formatting (cargo fmt --check)..."
cargo fmt --all -- --check

# ── 2. Rust lint — strict ───────────────────────────────────────
echo "  Linting Rust (cargo clippy — strict)..."
cargo clippy --all-targets --all-features --locked -- -D warnings

# ── 3. Rust tests ───────────────────────────────────────────────
echo "  Running unit tests (cargo test)..."
cargo test --locked --no-fail-fast

# ── 4. Build check ──────────────────────────────────────────────
echo "  Build check (cargo check --locked)..."
cargo check --locked --all-targets

# ── 5. Frontend — format + lint (verify only) ───────────────────
echo "  Checking React formatting (prettier --check)..."
(cd frontend && npx prettier --check src)

echo "  Linting React (eslint)..."
(cd frontend && npm run lint)

echo "All checks passed."
