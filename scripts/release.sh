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

# Check for clean working tree
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: Working tree is dirty. Commit or stash changes first."
    exit 1
fi

# Run checks before releasing
echo "Running checks..."
cargo fmt --all -- --check || { echo "Error: formatting issues. Run 'cargo fmt' first."; exit 1; }
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

read -rp "Push to origin and trigger release build? [Y/n] " confirm < /dev/tty
if [[ "${confirm:-Y}" =~ ^[Yy]$ ]]; then
    git push && git push --tags
    echo ""
    COMMIT_HASH=$(git rev-parse HEAD)
    echo "Done! Release workflow will build and publish artifacts."
    echo "Watch progress at: https://github.com/markovic-nikola/tuxflow/actions"
    echo ""
    echo "Flathub: update commit hash in com.tuxflow.TuxFlow.yml to:"
    echo "  commit: $COMMIT_HASH"
    echo "Then regenerate cargo-sources.json and push to the Flathub repo."
else
    echo ""
    echo "Committed and tagged locally. Push when ready:"
    echo "  git push && git push --tags"
fi
