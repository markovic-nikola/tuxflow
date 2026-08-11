#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Read current version from Cargo.toml and bump patch
CURRENT=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"
NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))"

echo "Current version: $CURRENT"
echo "New version:     $NEW_VERSION"
echo ""

# Refuse to release from a dirty tree. The cargo fmt step below commits
# whatever it finds as "cargo fmt", so any unrelated work left uncommitted
# gets swallowed under that message — and tagged and pushed seconds later,
# where fixing the message costs a force-push. Covers staged, unstaged and
# untracked in one check.
if [ -n "$(git status --porcelain)" ]; then
    echo "Error: uncommitted changes. Commit or stash them before releasing."
    git status --short
    exit 1
fi

# Run checks before releasing
echo "Running checks..."
cargo fmt --all
# The tree was clean above, so anything dirty now is cargo fmt's own doing
# and the message is honest.
if ! git diff --quiet; then
    git add -A
    git commit -m "cargo fmt"
    echo "  Auto-committed formatting fixes"
fi
cargo clippy --all-targets -- -W clippy::all 2>&1 | grep -q "^error" && { echo "Error: clippy errors found."; exit 1; }
cargo test --all || { echo "Error: tests failed."; exit 1; }
echo "  All checks passed."
echo ""

TODAY=$(date +%Y-%m-%d)

echo "Releasing v$NEW_VERSION..."
echo ""

# 1. Cargo.toml
sed -i "0,/^version = \".*\"/s//version = \"$NEW_VERSION\"/" Cargo.toml
echo "  Updated Cargo.toml"

# 2. Metainfo — insert new release entry after <releases>
sed -i "/<releases>/a\\    <release version=\"$NEW_VERSION\" date=\"$TODAY\">\n      <description>\n        <p>Release $NEW_VERSION.</p>\n      </description>\n    </release>" data/com.tuxflow.TuxFlow.metainfo.xml
echo "  Updated data/com.tuxflow.TuxFlow.metainfo.xml"

# 3. Update Cargo.lock
cargo generate-lockfile 2>/dev/null || true
echo "  Updated Cargo.lock"

# 4. Commit, tag, push
echo ""
git add -A
git commit -m "release v$NEW_VERSION"
git tag "v$NEW_VERSION"
echo ""

git push && git push --tags
echo ""
echo "Done! Release workflow will build and publish artifacts."
echo "Watch progress at: https://github.com/markovic-nikola/tuxflow/actions"
