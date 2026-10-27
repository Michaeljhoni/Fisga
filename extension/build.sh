#!/usr/bin/env bash
# Builds the unpacked extensions:
#   extension/dist/chrome   -> chrome://extensions, "Load unpacked" (Chrome, Chromium, Brave, Edge, Vivaldi)
#   extension/dist/firefox  -> about:debugging, "Load Temporary Add-on" (pick manifest.json)
# and, when zip is installed, the packages sent to the stores (not installed by the packages):
#   extension/dist-store/fisga-chrome-<version>.zip   -> Chrome Web Store
#   extension/dist-store/fisga-firefox-<version>.zip  -> addons.mozilla.org
set -euo pipefail
cd "$(dirname "$0")"
rm -rf dist dist-store
for browser in chrome firefox; do
  mkdir -p "dist/$browser"
  cp -r src/. "dist/$browser/"
  cp "manifest.$browser.json" "dist/$browser/manifest.json"
done
if command -v zip >/dev/null; then
  version="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' manifest.chrome.json)"
  mkdir -p dist-store/chrome
  cp -r dist/chrome/. dist-store/chrome/
  # The store refuses "key": it gives the extension its own.
  grep -v '^  "key": ' manifest.chrome.json > dist-store/chrome/manifest.json
  (cd dist-store/chrome && zip -qrX "../fisga-chrome-$version.zip" .)
  (cd dist/firefox && zip -qrX "../../dist-store/fisga-firefox-$version.zip" .)
  rm -rf dist-store/chrome
  echo "store packages in $(pwd)/dist-store"
fi
echo "extension built in $(pwd)/dist"
