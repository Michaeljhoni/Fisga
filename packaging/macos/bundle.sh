#!/usr/bin/env bash
# Builds dist/macos/Fisga.app and dist/macos/Fisga-<version>.dmg: Fisga's
# programs with the GTK 4 they need inside the app. Run on macOS after
# `cargo build --release --workspace` and `bash extension/build.sh`, with
# Homebrew's gtk4, librsvg, adwaita-icon-theme and dylibbundler.
#
# The app is signed ad hoc only (no Apple Developer ID): the first time,
# macOS asks to allow it in System Settings > Privacy & Security.
set -euo pipefail
shopt -s nullglob
cd "$(dirname "$0")/../.."
prefix="$(brew --prefix)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
out=dist/macos
app="$out/Fisga.app"
contents="$app/Contents"
resources="$contents/Resources"
rm -rf "$out"
mkdir -p "$contents/MacOS" "$contents/Frameworks" "$resources"

cp target/release/{fisga,fisgad,fisga-gui,fisga-native-host} "$contents/MacOS/"

# Image loaders (the icons are SVG). Their list names them by absolute path,
# which depends on where the app is put: fisga-gui writes the real one at
# start from this template (see use_bundle_resources in its main.rs).
loaders=lib/gdk-pixbuf-2.0/2.10.0
mkdir -p "$resources/$loaders/loaders"
gdk-pixbuf-query-loaders > "$out/loaders.cache"
sed -n 's#^"\(/[^"]*/loaders/[^"/]*\)"$#\1#p' "$out/loaders.cache" | while read -r module; do
  cp "$module" "$resources/$loaders/loaders/"
done
sed 's#^"/[^"]*/loaders/\([^"/]*\)"$#"@RESOURCES@/'"$loaders"'/loaders/\1"#' "$out/loaders.cache" \
  > "$resources/$loaders/loaders.cache.in"
rm "$out/loaders.cache"

# Every Homebrew library the programs and loaders use, into Frameworks, with
# the references rewritten to point there. Some are named by @rpath (librsvg,
# from the SVG loader): they are in Homebrew's lib. A library it still cannot
# find makes dylibbundler ask for its folder, over and over: "quit" ends it.
args=()
for file in "$contents"/MacOS/* "$resources/$loaders"/loaders/*; do
  args+=(-x "$file")
done
dylibbundler -of -b -cd "${args[@]}" -s "$prefix/lib" -d "$contents/Frameworks" -p @executable_path/../Frameworks/ <<< quit

# GTK's settings schemas, its icons (Adwaita) and Fisga's own.
schemas="$resources/share/glib-2.0/schemas"
mkdir -p "$schemas"
cp "$prefix"/share/glib-2.0/schemas/org.gtk.gtk4.*.xml "$schemas/"
glib-compile-schemas "$schemas"
icons="$resources/share/icons"
mkdir -p "$icons/hicolor/scalable/apps"
cp -R "$prefix/share/icons/Adwaita" "$icons/"
cp "$prefix/share/icons/hicolor/index.theme" "$icons/hicolor/"
for size in 32 48 64 128 256; do
  mkdir -p "$icons/hicolor/${size}x${size}/apps"
  cp "crates/fisga-gui/data/icons/$size/io.fisga.Fisga.png" "$icons/hicolor/${size}x${size}/apps/"
done
cp crates/fisga-gui/data/io.fisga.Fisga.svg "$icons/hicolor/scalable/apps/"
for theme in Adwaita hicolor; do
  gtk4-update-icon-cache -q -t -f "$icons/$theme"
done

# The app's icon, from the PNGs.
iconset="$out/Fisga.iconset"
mkdir -p "$iconset"
png() { echo "crates/fisga-gui/data/icons/$1/io.fisga.Fisga.png"; }
sips -z 16 16 "$(png 32)" --out "$iconset/icon_16x16.png" > /dev/null
cp "$(png 32)" "$iconset/icon_16x16@2x.png"
cp "$(png 32)" "$iconset/icon_32x32.png"
cp "$(png 64)" "$iconset/icon_32x32@2x.png"
cp "$(png 128)" "$iconset/icon_128x128.png"
cp "$(png 256)" "$iconset/icon_128x128@2x.png"
cp "$(png 256)" "$iconset/icon_256x256.png"
iconutil -c icns "$iconset" -o "$resources/Fisga.icns"
rm -r "$iconset"

# The extension unpacked, for loading it by hand; and the license.
mkdir -p "$resources/share/fisga/extension"
cp -R extension/dist/chrome extension/dist/firefox "$resources/share/fisga/extension/"
cp LICENSE "$resources/"

cat > "$contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>io.fisga.Fisga</string>
	<key>CFBundleName</key>
	<string>Fisga</string>
	<key>CFBundleDisplayName</key>
	<string>Fisga</string>
	<key>CFBundleExecutable</key>
	<string>fisga-gui</string>
	<key>CFBundleIconFile</key>
	<string>Fisga</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$version</string>
	<key>CFBundleVersion</key>
	<string>$version</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>GPL-3.0-or-later</string>
</dict>
</plist>
EOF

# Rewriting the libraries' references broke their signatures; Apple silicon
# runs nothing unsigned.
codesign --force --deep --sign - "$app"

stage="$out/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname Fisga -srcfolder "$stage" -ov -format UDZO "$out/Fisga-$version.dmg"
rm -r "$stage"
du -sh "$app" "$out/Fisga-$version.dmg"
