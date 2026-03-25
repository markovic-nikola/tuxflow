#!/usr/bin/env bash
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "Usage: $0 <new-version>"
    echo "Example: $0 0.2.0"
    exit 1
fi

NEW_VERSION="$1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if ! [[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in semver format (e.g., 0.2.0)"
    exit 1
fi

echo "Bumping version to $NEW_VERSION..."

# Cargo.toml (first version = line only)
sed -i "0,/^version = \".*\"/s//version = \"$NEW_VERSION\"/" "$ROOT/Cargo.toml"
echo "  Updated Cargo.toml"

# AUR PKGBUILD
sed -i "s/^pkgver=.*/pkgver=$NEW_VERSION/" "$ROOT/packaging/aur/PKGBUILD"
echo "  Updated packaging/aur/PKGBUILD"

# AppImage (if exists)
if [ -f "$ROOT/packaging/appimage/AppImageBuilder.yml" ]; then
    sed -i "s/version: .*/version: $NEW_VERSION/" "$ROOT/packaging/appimage/AppImageBuilder.yml"
    echo "  Updated packaging/appimage/AppImageBuilder.yml"
fi

# Update Cargo.lock
(cd "$ROOT" && cargo generate-lockfile 2>/dev/null || true)
echo "  Updated Cargo.lock"

echo ""
echo "MANUAL STEP: Add a <release> entry to data/com.tuxflow.TuxFlow.metainfo.xml:"
echo "  <release version=\"$NEW_VERSION\" date=\"$(date +%Y-%m-%d)\">"
echo "    <description><p>...</p></description>"
echo "  </release>"
echo ""
echo "Next steps:"
echo "  1. Edit metainfo.xml release notes"
echo "  2. Run 'cargo build' to verify"
echo "  3. Commit and tag: git tag v$NEW_VERSION"
echo "  4. Push: git push && git push --tags"
