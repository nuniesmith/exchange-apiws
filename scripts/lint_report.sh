#!/usr/bin/env bash
# lint_report.sh — Compile (if needed) and run the Rust lint_report binary.
#
# Runs: cargo fmt --check, cargo clippy, cargo test, cargo doc
# and writes a Markdown report summarising all results.
#
# Usage: ./scripts/lint_report.sh [workspace_dir] [output_file]
#   workspace_dir  root of the Rust workspace  (default: repo root)
#   output_file    report path                  (default: lint_report.md)
set -euo pipefail

# ── paths ──────────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

WORKSPACE_DIR="${1:-$REPO_ROOT}"
OUTPUT="${2:-$REPO_ROOT/lint_report.md}"

RUST_SRC="$SCRIPT_DIR/lint_report.rs"
BINARY="$SCRIPT_DIR/lint_report_bin"

# ── helpers ────────────────────────────────────────────────────────────────────
die() { echo "ERROR: $*" >&2; exit 1; }

# ── pre-flight ─────────────────────────────────────────────────────────────────
[ -d "$WORKSPACE_DIR" ] || die "Workspace directory '$WORKSPACE_DIR' not found."
[ -f "$WORKSPACE_DIR/Cargo.toml" ] || die "'$WORKSPACE_DIR' does not look like a Rust workspace (no Cargo.toml)."
[ -f "$RUST_SRC" ] || die "Rust source '$RUST_SRC' not found."

command -v cargo  &>/dev/null || die "cargo not found — install Rust from https://rustup.rs"
command -v rustc  &>/dev/null || die "rustc not found — install Rust from https://rustup.rs"
command -v rustfmt &>/dev/null || die "rustfmt not found — run: rustup component add rustfmt"

# clippy is invoked as `cargo clippy`, not as a standalone binary.
cargo clippy --version &>/dev/null || die "clippy not found — run: rustup component add clippy"

# ── build (only when source is newer than binary) ──────────────────────────────
if [ ! -f "$BINARY" ] || [ "$RUST_SRC" -nt "$BINARY" ]; then
    echo "Building lint_report binary…"
    rustc -O --edition 2021 "$RUST_SRC" -o "$BINARY"
    echo "Build complete."
else
    echo "lint_report binary is up to date, skipping build."
fi

# ── ensure output directory exists ────────────────────────────────────────────
mkdir -p "$(dirname "$OUTPUT")"

# ── run ────────────────────────────────────────────────────────────────────────
echo "Running lint checks on '$WORKSPACE_DIR'…"

# Disable set -e around the binary call so we can capture its exit code
# and still print a useful summary line before propagating it.
set +e
"$BINARY" "$WORKSPACE_DIR" "$OUTPUT"
EXIT_CODE=$?
set -e

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo "✅ All checks passed. Report: $OUTPUT"
else
    echo "❌ One or more checks failed. Report: $OUTPUT"
fi

exit $EXIT_CODE
