#!/usr/bin/env bash
# release.sh — validate, tag, and publish the version already set in Cargo.toml.
#
# This crate's workflow sets the target version in Cargo.toml as part of each
# feature PR, so by release time Cargo.toml is the source of truth. This
# script does NOT invent a new version — it validates the current state,
# tags it, pushes, and publishes.
#
# Usage:
#   ./scripts/release.sh [--dry-run] [--bump-patch] [--allow-dirty]
#
#   --dry-run      Print every mutating step instead of running it.
#   --bump-patch   Increment the Cargo.toml patch version first (X.Y.Z -> X.Y.Z+1)
#                  and commit it, then release that. Use when you haven't already
#                  bumped the version in a PR.
#   --allow-dirty  Skip the clean-working-tree check (passed through to
#                  `cargo publish` as well).
set -euo pipefail

DRY_RUN=false
BUMP_PATCH=false
ALLOW_DIRTY=false
for arg in "$@"; do
    case "$arg" in
        --dry-run)     DRY_RUN=true ;;
        --bump-patch)  BUMP_PATCH=true ;;
        --allow-dirty) ALLOW_DIRTY=true ;;
        *) echo "ERROR: unknown flag '$arg'"; exit 1 ;;
    esac
done
$DRY_RUN && echo "==> DRY RUN — no changes will be made"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

run() {
    if $DRY_RUN; then
        echo "[dry-run] $*"
    else
        "$@"
    fi
}

die() { echo "ERROR: $*" >&2; exit 1; }

CARGO="Cargo.toml"
[[ -f "$CARGO" ]] || die "$CARGO not found — run from the crate root"

# ── 1. Optional patch bump ────────────────────────────────────────────────────
if $BUMP_PATCH; then
    CUR=$(grep -E '^version\s*=' "$CARGO" | head -n1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
    IFS='.' read -r MA MI PA <<< "$CUR"
    NEW="${MA}.${MI}.$(( PA + 1 ))"
    echo "==> Bumping Cargo.toml version: $CUR -> $NEW"
    if ! $DRY_RUN; then
        sed -i "0,/^\(version\s*=\s*\)\"[^\"]*\"/s//\1\"${NEW}\"/" "$CARGO"
    fi
    run git add "$CARGO"
    run git commit -m "chore: bump version to ${NEW}"
fi

# ── 2. Read the version from Cargo.toml (source of truth) ─────────────────────
VERSION=$(grep -E '^version\s*=' "$CARGO" | head -n1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
[[ -n "$VERSION" ]] || die "could not parse version from $CARGO"
TAG="v${VERSION}"
echo "==> Releasing version $VERSION  (tag: $TAG)"

# ── 3. Pre-flight validation ──────────────────────────────────────────────────

# 3a. Working tree must be clean (unless --allow-dirty).
if ! $ALLOW_DIRTY && [[ -n "$(git status --porcelain)" ]]; then
    die "working tree is dirty — commit/stash first, or pass --allow-dirty"
fi

# 3b. The tag must not already exist (avoid re-releasing a version).
if git rev-parse "$TAG" >/dev/null 2>&1; then
    die "tag $TAG already exists — bump the version in $CARGO before releasing"
fi

# 3c. CHANGELOG must have an entry for this version (Keep a Changelog format).
if [[ -f CHANGELOG.md ]]; then
    grep -qE "^##\s*\[${VERSION}\]" CHANGELOG.md \
        || die "CHANGELOG.md has no '## [${VERSION}]' section — document the release first"
    echo "==> CHANGELOG entry for ${VERSION} found"
else
    echo "==> (no CHANGELOG.md — skipping changelog check)"
fi

# 3d. Build, test, and package must all succeed. These run regardless of
#     --dry-run; a release that can't build/test/package shouldn't proceed.
echo "==> cargo build --release"
cargo build --release --quiet || die "release build failed"

echo "==> cargo test (all default features)"
cargo test --quiet || die "tests failed"

echo "==> cargo package (builds + verifies the publish bundle without uploading)"
PACKAGE_FLAGS=()
$ALLOW_DIRTY && PACKAGE_FLAGS+=(--allow-dirty)
cargo package --quiet "${PACKAGE_FLAGS[@]}" || die "cargo package failed — check excluded/missing files"

echo "==> pre-flight checks passed"

# ── 4. Tag, push, publish ─────────────────────────────────────────────────────
run git tag -a "$TAG" -m "Release ${TAG}"

BRANCH=$(git rev-parse --abbrev-ref HEAD)
echo "==> Pushing branch '$BRANCH' and tag '$TAG'"
run git push origin "$BRANCH"
run git push origin "$TAG"

echo "==> Publishing to crates.io"
if $ALLOW_DIRTY; then
    run cargo publish --allow-dirty
else
    run cargo publish
fi

echo ""
echo "✓ Released ${TAG}"
