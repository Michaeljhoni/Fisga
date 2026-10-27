#!/usr/bin/env bash
# Sets Fisga's version everywhere it is written down:
#   bash scripts/bump-version.sh 1.2.0
# Cargo.toml and Cargo.lock (CI builds with --locked), the extension manifests and
# the fisga-bin PKGBUILD. Then write the version's section in CHANGELOG.md.
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:?usage: bump-version.sh X.Y.Z}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not X.Y.Z: $version" >&2; exit 1; }

sed -i "0,/^version = \".*\"$/s//version = \"$version\"/" Cargo.toml
# The workspace crates' entries: the version line right after a fisga-* name.
sed -i "/^name = \"fisga-[a-z-]*\"$/{n;s/^version = \".*\"$/version = \"$version\"/}" Cargo.lock
sed -i "s/^  \"version\": \".*\",$/  \"version\": \"$version\",/" extension/manifest.chrome.json extension/manifest.firefox.json
sed -i "s/^pkgver=.*/pkgver=$version/; s/^pkgrel=.*/pkgrel=1/" packaging/arch-bin/PKGBUILD

grep -q "^## $version\$" CHANGELOG.md || echo "CHANGELOG.md has no \"## $version\" section yet"
git diff --stat
