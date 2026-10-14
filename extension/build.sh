#!/usr/bin/env bash
# Builds the unpacked extensions:
#   extension/dist/chrome   -> chrome://extensions, "Load unpacked" (Chrome, Chromium, Brave, Edge, Vivaldi)
#   extension/dist/firefox  -> about:debugging, "Load Temporary Add-on" (pick manifest.json)
set -euo pipefail
cd "$(dirname "$0")"
rm -rf dist
for browser in chrome firefox; do
  mkdir -p "dist/$browser"
  cp -r src/. "dist/$browser/"
  cp "manifest.$browser.json" "dist/$browser/manifest.json"
done
if command -v zip >/dev/null; then
  (cd dist/firefox && zip -qr ../ldm-firefox.xpi .)
  (cd dist/chrome && zip -qr ../ldm-chrome.zip .)
fi
echo "extension built in $(pwd)/dist"
