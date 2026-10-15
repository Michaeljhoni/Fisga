#!/usr/bin/env bash
# GUI smoke test under Xvfb: daemon launches `ldm-gui --pending`, the "Novo download"
# window appears, Enter = "Baixar agora" opens the progress window, which turns
# into "Download concluído"; Escape = "Cancelar"; then the main window
# (sidebar with categories) and Preferências.
# Usage: dbus-run-session -- scripts/ci/gui-smoke.sh [bin-dir] [screenshot-dir]
# Needs Xvfb, xdotool and ImageMagick; the bin dir must contain examples/range_server.
set -euo pipefail
BIN="${1:-target/debug}"
BIN="$(cd "$BIN" && pwd)"
OUT="${2:-target/gui-shots}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
mkdir -p "$OUT"
SANDBOX="$(mktemp -d)"
export HOME="$SANDBOX/home" XDG_RUNTIME_DIR="$SANDBOX/run" XDG_DATA_HOME="$SANDBOX/data" XDG_CONFIG_HOME="$SANDBOX/home/.config"
mkdir -p "$HOME/Downloads" "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
# Fake default app for .bin files: records what "Abrir" asked to open.
mkdir -p "$XDG_DATA_HOME/applications" "$XDG_CONFIG_HOME"
printf '#!/bin/sh
echo "$1" > "%s/opened"
' "$SANDBOX" > "$SANDBOX/fake-open"
chmod +x "$SANDBOX/fake-open"
printf '[Desktop Entry]
Type=Application
Name=Abridor de teste
Exec=%s %%f
MimeType=application/octet-stream;
'   "$SANDBOX/fake-open" > "$XDG_DATA_HOME/applications/ldm-test-open.desktop"
printf '[Default Applications]
application/octet-stream=ldm-test-open.desktop
' > "$XDG_CONFIG_HOME/mimeapps.list"
export DISPLAY=:99 GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1
Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB=$!
sleep 1

"$BIN/examples/range_server" 0 20 > "$SANDBOX/server.url" &
SERVER=$!
trap 'kill $XVFB $SERVER 2>/dev/null; "$BIN/ldm" stop >/dev/null 2>&1 || true' EXIT
for _ in $(seq 1 50); do [ -s "$SANDBOX/server.url" ] && break; sleep 0.1; done
BASE="$(cat "$SANDBOX/server.url")"
URL="$BASE/files/52428800.bin"

# Native messaging framing: 32-bit little-endian length (in bytes, not characters) + JSON.
frame() {
  local n; n=$(printf %s "$1" | wc -c)
  printf "\x$(printf %02x $((n & 255)))\x$(printf %02x $(((n >> 8) & 255)))\x00\x00%s" "$1"
}
host() { frame "$1" | "$BIN/ldm-native-host" | tail -c +5; echo; }
state_of() { "$BIN/ldm" list | awk -v id="$1" '$1 == id { print $2 }'; }
find_window() { for _ in $(seq 1 75); do w="$(xdotool search --name "$1" 2>/dev/null | head -1)"; [ -n "$w" ] && { echo "$w"; return 0; }; sleep 0.2; done; echo "window '$1' not found" >&2; return 1; }
shot() { import -window root "$OUT/$1.png"; echo "screenshot $1"; }

echo "== extension click -> pending + ldm-gui --pending"
host "{\"type\":\"download\",\"url\":\"$URL\",\"kind\":\"direct\",\"title\":\"Vídeo de teste\"}"
win="$(find_window "^Novo download$")"
pgrep -af "ldm-gui --pending"
sleep 1.5   # let the probe fill in the size
shot 1-novo-download
test "$(state_of 1)" = aguardando
xdotool windowfocus --sync "$win" key Return

echo "== progress window"
prog="$(find_window "52428800.bin")"
echo "progress window: $(xdotool getwindowname "$prog")"
sleep 3
shot 2-progresso
for _ in $(seq 1 120); do [ "$(state_of 1)" = concluído ] && break; sleep 0.5; done
test "$(state_of 1)" = concluído
test -f "$HOME/Downloads/Programas/52428800.bin"; echo "saved in Downloads/Programas"
done_win="$(find_window "^Download concluído$")"
sleep 1
shot 3-concluido
# Enter = "Abrir": the file opens in its default app, then the window closes.
xdotool windowfocus --sync "$done_win" key Return
for _ in $(seq 1 50); do [ -s "$SANDBOX/opened" ] && break; sleep 0.2; done
grep -q "Programas/52428800.bin" "$SANDBOX/opened"; echo "Abrir opened $(cat "$SANDBOX/opened")"
for _ in $(seq 1 25); do xdotool search --name "^Download concluído$" >/dev/null 2>&1 || break; sleep 0.2; done
if xdotool search --name "^Download concluído$" >/dev/null 2>&1; then echo "window still open"; exit 1; fi
echo "window closed after opening"

echo "== Escape cancels"
host "{\"type\":\"download\",\"url\":\"$BASE/files/1048576.bin\",\"kind\":\"direct\"}"
win="$(find_window "^Novo download$")"
xdotool windowfocus --sync "$win" key Escape
for _ in $(seq 1 30); do [ -z "$(state_of 2)" ] && break; sleep 0.2; done
test -z "$(state_of 2)"; echo "download 2 removed"
sleep 4.5
if pgrep -f "ldm-gui --pending" >/dev/null; then echo "ldm-gui still running"; xdotool search --name . getwindowname %@ 2>/dev/null || true; exit 1; fi
echo "ldm-gui --pending quit after the windows closed"

echo "== main window"
"$BIN/ldm" add "$BASE/files/2097152.bin" -n foto.png --wait >/dev/null
host "{\"type\":\"download\",\"url\":\"$BASE/files/4194304.bin\",\"kind\":\"direct\"}"
sleep 2
"$BIN/ldm-gui" &
main="$(find_window "^LDM$")"
pending_count="$(xdotool search --name "^Novo download$" | wc -l)"
test "$pending_count" -eq 1; echo "pending window for #4 is open too"
xdotool windowfocus --sync "$(xdotool search --name '^Novo download$' | head -1)" key Escape
sleep 1
xdotool windowfocus --sync "$main"
sleep 1
shot 4-janela-principal
xdotool windowfocus --sync "$main" key ctrl+comma
prefs="$(find_window "^Preferências$")"
sleep 1
shot 5-preferencias
xdotool windowfocus --sync "$prefs" key Escape || true
"$BIN/ldm" list
ls -R "$HOME/Downloads"
echo "GUI SMOKE OK"
