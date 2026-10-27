#!/usr/bin/env bash
# Lays out Fisga's files under ROOT the way a package installs them (usr/, etc/):
#   bash scripts/stage.sh ROOT
# Takes the release binaries from target/release and the extension from
# extension/dist, so run cargo build --release and extension/build.sh first.
# Used by both Arch packages (package()) and the release tarballs.
set -euo pipefail
root="$(realpath -m "${1:?usage: stage.sh ROOT}")"
cd "$(dirname "$0")/.."

install -Dm755 target/release/fisga target/release/fisgad target/release/fisga-native-host target/release/fisga-gui \
  -t "$root/usr/bin"
install -Dm644 crates/fisga-gui/data/io.fisga.Fisga.desktop -t "$root/usr/share/applications"
install -Dm644 crates/fisga-gui/data/io.fisga.Fisga.svg -t "$root/usr/share/icons/hicolor/scalable/apps"
for size in 32 48 64 128 256; do
  install -Dm644 "crates/fisga-gui/data/icons/$size/io.fisga.Fisga.png" -t "$root/usr/share/icons/hicolor/${size}x${size}/apps"
done
# The browser host for every user (Firefox, Chromium, Chrome, Brave, Edge, Vivaldi, LibreWolf).
target/release/fisga install-hosts --system "$root" --host-path /usr/bin/fisga-native-host
# The extension, to load unpacked: chrome://extensions or about:debugging.
install -d "$root/usr/share/fisga"
cp -r extension/dist "$root/usr/share/fisga/extension"
install -Dm644 README.md CHANGELOG.md docs/GUIA.md docs/CONTINUAR.md -t "$root/usr/share/doc/fisga"
