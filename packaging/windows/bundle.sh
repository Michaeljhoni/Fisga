#!/usr/bin/env bash
# Lays out dist/windows/Fisga: Fisga's programs with the GTK 4 they need, as
# GTK expects to find itself on Windows (bin, lib and share side by side).
# Run in MSYS2's UCRT64 shell after `cargo build --release --workspace` and
# `bash extension/build.sh`; packaging/windows/fisga.iss turns it into the installer.
set -euo pipefail
cd "$(dirname "$0")/../.."
prefix="${MINGW_PREFIX:-/ucrt64}"
out=dist/windows/Fisga
rm -rf "$out"
mkdir -p "$out/bin"

cp target/release/{fisga,fisgad,fisga-gui,fisga-native-host}.exe "$out/bin/"

# Image loaders (the icons are SVG) with their list, which names them relative
# to the install folder.
loaders=lib/gdk-pixbuf-2.0/2.10.0
mkdir -p "$out/$loaders/loaders"
cp "$prefix/$loaders"/loaders/*.dll "$out/$loaders/loaders/"
cp "$prefix/$loaders/loaders.cache" "$out/$loaders/"

# Every MSYS2 DLL the programs and loaders use (ldd lists them all, also the
# indirect ones); Windows' own stay out.
ldd "$out"/bin/*.exe "$out/$loaders"/loaders/*.dll \
  | awk -v prefix="$prefix/" 'index($3, prefix) == 1 { print $3 }' \
  | sort -u \
  | xargs -r cp -t "$out/bin/"

# GTK's settings schemas (the file chooser reads them).
schemas="$out/share/glib-2.0/schemas"
mkdir -p "$schemas"
cp "$prefix"/share/glib-2.0/schemas/org.gtk.gtk4.*.xml "$schemas/"
glib-compile-schemas "$schemas"

# Icons: GTK's symbolic ones (Adwaita) and Fisga's own.
icons="$out/share/icons"
mkdir -p "$icons"
cp -r "$prefix/share/icons/Adwaita" "$icons/"
mkdir -p "$icons/hicolor"
cp "$prefix/share/icons/hicolor/index.theme" "$icons/hicolor/"
for size in 32 48 64 128 256; do
  mkdir -p "$icons/hicolor/${size}x${size}/apps"
  cp "crates/fisga-gui/data/icons/$size/io.fisga.Fisga.png" "$icons/hicolor/${size}x${size}/apps/"
done
mkdir -p "$icons/hicolor/scalable/apps"
cp crates/fisga-gui/data/io.fisga.Fisga.svg "$icons/hicolor/scalable/apps/"
for theme in Adwaita hicolor; do
  gtk4-update-icon-cache -q -t -f "$icons/$theme"
done

# The extension unpacked, for loading it by hand (until it is in the Chrome
# Web Store); and the license.
mkdir -p "$out/share/fisga/extension"
cp -r extension/dist/chrome extension/dist/firefox "$out/share/fisga/extension/"
cp LICENSE "$out/"

du -sh "$out"
