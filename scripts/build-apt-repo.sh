#!/usr/bin/env bash
# Build a signed apt repository from one or more .deb files.
#
#   build-apt-repo.sh OUTDIR DEB [DEB...]
#
# Layout produced (served from the gh-pages branch root):
#   KEY.gpg                                   public signing key, for apt setup
#   dists/stable/Release{,.gpg} InRelease     signed indices
#   dists/stable/main/binary-amd64/Packages{,.gz}
#   pool/main/t/tuxflow/*.deb
#
# Signing uses whatever secret key gpg has; CI imports it from a secret first.
# Set APT_SIGN=0 to build unsigned indices (local testing only — apt refuses
# unsigned repos without an explicit [trusted=yes]).
set -euo pipefail

OUT="${1:?usage: build-apt-repo.sh OUTDIR DEB [DEB...]}"
shift
[ $# -gt 0 ] || { echo "error: no .deb files given" >&2; exit 1; }

ORIGIN="TuxFlow"
SUITE="stable"
COMPONENT="main"
ARCH="amd64"

mkdir -p "$OUT/dists/$SUITE/$COMPONENT/binary-$ARCH" "$OUT/pool/$COMPONENT/t/tuxflow"
for deb in "$@"; do
    cp -f "$deb" "$OUT/pool/$COMPONENT/t/tuxflow/"
done

cd "$OUT"

# Paths inside Packages are relative to the repo root, so run from it.
apt-ftparchive --arch "$ARCH" packages pool \
    > "dists/$SUITE/$COMPONENT/binary-$ARCH/Packages"
gzip -9cn "dists/$SUITE/$COMPONENT/binary-$ARCH/Packages" \
    > "dists/$SUITE/$COMPONENT/binary-$ARCH/Packages.gz"

apt-ftparchive release \
    -o APT::FTPArchive::Release::Origin="$ORIGIN" \
    -o APT::FTPArchive::Release::Label="$ORIGIN" \
    -o APT::FTPArchive::Release::Suite="$SUITE" \
    -o APT::FTPArchive::Release::Codename="$SUITE" \
    -o APT::FTPArchive::Release::Architectures="$ARCH" \
    -o APT::FTPArchive::Release::Components="$COMPONENT" \
    -o APT::FTPArchive::Release::Description="TuxFlow apt repository" \
    "dists/$SUITE" > "dists/$SUITE/Release"

if [ "${APT_SIGN:-1}" = "1" ]; then
    # InRelease (inline-signed) is what modern apt fetches; Release.gpg is the
    # detached fallback for older clients.
    gpg --batch --yes --pinentry-mode loopback \
        --clearsign -o "dists/$SUITE/InRelease" "dists/$SUITE/Release"
    gpg --batch --yes --pinentry-mode loopback \
        --detach-sign --armor -o "dists/$SUITE/Release.gpg" "dists/$SUITE/Release"
    gpg --armor --export > KEY.gpg
    echo "signed with: $(gpg --list-secret-keys --keyid-format=long | grep -m1 sec || true)"
else
    echo "APT_SIGN=0 — indices left unsigned"
fi

echo "apt repo built in $OUT:"
find . -maxdepth 4 -type f | sort | sed 's/^/  /'
