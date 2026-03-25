#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ $# -ne 1 ]; then
    echo "Usage: $0 <new-version>"
    echo "Example: $0 0.2.0"
    exit 1
fi

NEW_VERSION="$1"

if ! [[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in semver format (e.g., 0.2.0)"
    exit 1
fi

# Check for clean working tree
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: Working tree is dirty. Commit or stash changes first."
    exit 1
fi

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
    sed -i "s/version: .*/version: $NEW_VERSION/" packaging/appimage/AppImageBuilder.yml
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

read -rp "Push to origin and trigger release build? [Y/n] " confirm
if [[ "${confirm:-Y}" =~ ^[Yy]$ ]]; then
    git push && git push --tags
    echo ""
    echo "Done! Release workflow will build and publish artifacts."
    echo "Watch progress at: https://github.com/markovic-nikola/tuxflow/actions"
else
    echo ""
    echo "Committed and tagged locally. Push when ready:"
    echo "  git push && git push --tags"
fi
