#!/usr/bin/env bash
# Builds LDM, installs it in ~/.local/bin and registers the browser host.
# Works on any distro with a Rust toolchain (Arch: pacman -S rust; others: rustup).
set -euo pipefail
cd "$(dirname "$0")/.."

PREFIX="${PREFIX:-$HOME/.local}"
cargo build --release --workspace
install -Dm755 target/release/ldm target/release/ldmd target/release/ldm-native-host -t "$PREFIX/bin"
"$PREFIX/bin/ldm" install-hosts
bash extension/build.sh

missing=()
for tool in yt-dlp ffmpeg notify-send; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done

cat <<EOF

LDM instalado em $PREFIX/bin.

Extensão:
  Chrome/Chromium/Brave/Edge/Vivaldi: abra chrome://extensions, ative o "Modo do desenvolvedor",
    clique em "Carregar sem compactação" e escolha: $(pwd)/extension/dist/chrome
  Firefox: abra about:debugging#/runtime/this-firefox, "Carregar extensão temporária..." e escolha
    $(pwd)/extension/dist/firefox/manifest.json
EOF
if [ ${#missing[@]} -gt 0 ]; then
  echo
  echo "Opcional, para vídeos de sites (YouTube etc.) e notificações: instale ${missing[*]}"
  echo "  Arch: sudo pacman -S yt-dlp ffmpeg libnotify"
fi
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo; echo "Atenção: $PREFIX/bin não está no PATH." ;;
esac
