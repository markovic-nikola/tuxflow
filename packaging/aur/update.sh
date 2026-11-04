#!/usr/bin/env bash
# Points the AUR package at a published release: sets pkgver, resets pkgrel,
# fills in the tarball's checksum and regenerates .SRCINFO from the PKGBUILD.
# Run after the release workflow has uploaded the tarball.
#
#   packaging/aur/update.sh            # the latest release
#   packaging/aur/update.sh 0.2.26     # a specific one
set -euo pipefail
cd "$(dirname "$0")"

repo=markovic-nikola/tuxflow
version=${1:-$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" |
  sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')}
[ -n "$version" ] || { echo "Could not determine the release version" >&2; exit 1; }

url="https://github.com/$repo/releases/download/v$version/tuxflow-$version-x86_64-linux.tar.gz"
sum=$(curl -fsSL "$url" | sha256sum | cut -d' ' -f1)

sed -i -e "s/^pkgver=.*/pkgver=$version/" \
       -e "s/^pkgrel=.*/pkgrel=1/" \
       -e "s/^sha256sums=.*/sha256sums=('$sum')/" PKGBUILD

# What `makepkg --printsrcinfo` prints, without needing an Arch machine.
(
  # shellcheck disable=SC1091
  source ./PKGBUILD
  {
    echo "pkgbase = $pkgname"
    echo "	pkgdesc = $pkgdesc"
    echo "	pkgver = $pkgver"
    echo "	pkgrel = $pkgrel"
    echo "	url = $url"
    for v in "${arch[@]}"; do echo "	arch = $v"; done
    for v in "${license[@]}"; do echo "	license = $v"; done
    for v in "${depends[@]}"; do echo "	depends = $v"; done
    for v in "${optdepends[@]}"; do echo "	optdepends = $v"; done
    for v in "${provides[@]}"; do echo "	provides = $v"; done
    for v in "${conflicts[@]}"; do echo "	conflicts = $v"; done
    for v in "${options[@]}"; do echo "	options = $v"; done
    for v in "${source[@]}"; do echo "	source = $v"; done
    for v in "${sha256sums[@]}"; do echo "	sha256sums = $v"; done
    echo
    echo "pkgname = $pkgname"
  } > .SRCINFO
)

echo "PKGBUILD and .SRCINFO now point at v$version ($sum)"
