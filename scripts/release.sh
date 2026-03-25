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

# Check for staged changes
if ! git diff --cached --quiet; then
    echo "Error: Staged changes found. Commit or unstage them first."
    exit 1
fi

# Run checks before releasing
echo "Running checks..."
cargo fmt --all
# Auto-commit formatting changes if any
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

# 2. AUR PKGBUILD
sed -i "s/^pkgver=.*/pkgver=$NEW_VERSION/" packaging/aur/PKGBUILD
echo "  Updated packaging/aur/PKGBUILD"

# 3. AppImage (if exists)
if [ -f packaging/appimage/AppImageBuilder.yml ]; then
    sed -i "/app_info:/,/exec:/ s/version: .*/version: $NEW_VERSION/" packaging/appimage/AppImageBuilder.yml
    echo "  Updated packaging/appimage/AppImageBuilder.yml"
fi

# 4. Metainfo — insert new release entry after <releases>
sed -i "/<releases>/a\\    <release version=\"$NEW_VERSION\" date=\"$TODAY\">\n      <description>\n        <p>Release $NEW_VERSION.</p>\n      </description>\n    </release>" data/com.tuxflow.TuxFlow.metainfo.xml
echo "  Updated data/com.tuxflow.TuxFlow.metainfo.xml"

# 5. Update Cargo.lock
cargo generate-lockfile 2>/dev/null || true
echo "  Updated Cargo.lock"

# 6. Commit, tag, push
echo ""
git add -A
git commit -m "release v$NEW_VERSION"
git tag "v$NEW_VERSION"
echo ""

git push && git push --tags
echo ""
echo "Done! Release workflow will build and publish artifacts."
echo "Watch progress at: https://github.com/markovic-nikola/tuxflow/actions"
