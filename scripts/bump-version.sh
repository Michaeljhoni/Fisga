#!/usr/bin/env bash
# Sets Fisga's version everywhere it is written down:
#   bash scripts/bump-version.sh 1.2.0
# Cargo.toml and Cargo.lock (CI builds with --locked), the extension manifests and
# fisga-bin's PKGBUILD and .SRCINFO, with SKIP for the sums until the release
# workflow gives them. Then write the version's section in CHANGELOG.md.
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:?usage: bump-version.sh X.Y.Z}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not X.Y.Z: $version" >&2; exit 1; }

sed -i "0,/^version = \".*\"$/s//version = \"$version\"/" Cargo.toml
# The workspace crates' entries: the version line right after a fisga-* name.
sed -i "/^name = \"fisga-[a-z0-9-]*\"$/{n;s/^version = \".*\"$/version = \"$version\"/}" Cargo.lock
sed -i "s/^  \"version\": \".*\",$/  \"version\": \"$version\",/" extension/manifest.chrome.json extension/manifest.firefox.json
# The .SRCINFO as `makepkg --printsrcinfo` writes it (CI compares them); the
# old version, dots escaped, is replaced in its download addresses.
old="$(sed -n 's/^pkgver=//p' packaging/arch-bin/PKGBUILD)"
[[ -n "$old" ]] || { echo "no pkgver in packaging/arch-bin/PKGBUILD" >&2; exit 1; }
sed -i "s/^pkgver=.*/pkgver=$version/; s/^pkgrel=.*/pkgrel=1/; s/^\(sha256sums_[a-z0-9_]*\)=.*/\1=('SKIP')/" packaging/arch-bin/PKGBUILD
sed -i "s/^\tpkgver = .*/\tpkgver = $version/; s/^\tpkgrel = .*/\tpkgrel = 1/; /^\tsource_/s/${old//./\\.}/$version/g; s/^\t\(sha256sums_[a-z0-9_]*\) = .*/\t\1 = SKIP/" packaging/arch-bin/.SRCINFO

grep -q "^## $version\$" CHANGELOG.md || echo "CHANGELOG.md has no \"## $version\" section yet"
git diff --stat
