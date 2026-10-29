#!/usr/bin/env bash
# GUI smoke test under Xvfb: daemon launches `fisga-gui --pending`, the "Novo download"
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
'   "$SANDBOX/fake-open" > "$XDG_DATA_HOME/applications/fisga-test-open.desktop"
printf '[Default Applications]
application/octet-stream=fisga-test-open.desktop
' > "$XDG_CONFIG_HOME/mimeapps.list"
# The app's icon, as the package installs it: the window shows it in the header.
install -Dm644 "$(dirname "$0")/../../crates/fisga-gui/data/io.fisga.Fisga.svg" -t "$XDG_DATA_HOME/icons/hicolor/scalable/apps"
export DISPLAY=:99 GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 NO_AT_BRIDGE=1
Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB=$!
sleep 1

"$BIN/examples/range_server" 0 20 > "$SANDBOX/server.url" &
SERVER=$!
trap 'kill $XVFB $SERVER 2>/dev/null; "$BIN/fisga" stop >/dev/null 2>&1 || true' EXIT
for _ in $(seq 1 50); do [ -s "$SANDBOX/server.url" ] && break; sleep 0.1; done
BASE="$(cat "$SANDBOX/server.url")"
URL="$BASE/files/52428800.bin"

# Native messaging framing: 32-bit little-endian length (in bytes, not characters) + JSON.
frame() {
  local n; n=$(printf %s "$1" | wc -c)
  printf "\x$(printf %02x $((n & 255)))\x$(printf %02x $(((n >> 8) & 255)))\x00\x00%s" "$1"
}
host() { frame "$1" | "$BIN/fisga-native-host" | tail -c +5; echo; }
state_of() { "$BIN/fisga" list | awk -v id="$1" '$1 == id { print $2 }'; }
find_window() { for _ in $(seq 1 75); do w="$(xdotool search --name "$1" 2>/dev/null | head -1)"; [ -n "$w" ] && { echo "$w"; return 0; }; sleep 0.2; done; echo "window '$1' not found" >&2; return 1; }
shot() { import -window root "$OUT/$1.png"; echo "screenshot $1"; }

# A second queue: the "Fila" field shows in "Novo download", and "Filas" in the sidebar.
"$BIN/fisga" queue add Noite >/dev/null

echo "== extension click -> pending + fisga-gui --pending"
host "{\"type\":\"download\",\"url\":\"$URL\",\"kind\":\"direct\",\"title\":\"Vídeo de teste\"}"
win="$(find_window "^Novo download$")"
pgrep -af "fisga-gui --pending"
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

echo "== same file again: the window asks what to do with the existing one; Escape cancels"
host "{\"type\":\"download\",\"url\":\"$URL\",\"kind\":\"direct\"}"
win="$(find_window "^Novo download$")"
sleep 1.5
shot 1b-arquivo-ja-existe
xdotool windowfocus --sync "$win" key Escape
for _ in $(seq 1 30); do [ -z "$(state_of 2)" ] && break; sleep 0.2; done
test -z "$(state_of 2)"; echo "download 2 removed"
sleep 4.5
if pgrep -f "fisga-gui --pending" >/dev/null; then echo "fisga-gui still running"; xdotool search --name . getwindowname %@ 2>/dev/null || true; exit 1; fi
echo "fisga-gui --pending quit after the windows closed"

echo "== main window"
"$BIN/fisga" add "$BASE/files/2097152.bin" -n foto.png --wait >/dev/null
# Checked against its SHA-256: the list says so, and so does "Conferir soma".
foto="$(find "$HOME/Downloads" -name foto.png)"
"$BIN/fisga" verify "$("$BIN/fisga" list | awk '/foto.png/ { print $1 }')" "$(sha256sum "$foto")" | grep "confere"
host "{\"type\":\"download\",\"url\":\"$BASE/files/4194304.bin\",\"kind\":\"direct\"}"
sleep 2
"$BIN/fisga-gui" &
main="$(find_window "^Fisga$")"
pending_count="$(xdotool search --name "^Novo download$" | wc -l)"
test "$pending_count" -eq 1; echo "pending window for #4 is open too"
xdotool windowfocus --sync "$(xdotool search --name '^Novo download$' | head -1)" key Escape
sleep 1
xdotool windowfocus --sync "$main"
# A download running while the window is shot: the speed graph and the card's animations.
"$BIN/fisga" add "$BASE/files/104857600.bin" -o "$SANDBOX/running" >/dev/null
running="$("$BIN/fisga" list | awk '$1 ~ /^[0-9]+$/ && $1 > max { max = $1 } END { print max + 0 }')"
sleep 5
shot 4-janela-principal
"$BIN/fisga" pause "$running"
sleep 1
# Search: Ctrl+F, type, and only the matching row is left.
xdotool windowfocus --sync "$main" key ctrl+f
xdotool type --delay 30 "foto"
sleep 1
shot 4c-busca
# Right click on the first card, foto.png (the search left only it): its menu (Abrir, Abrir pasta, Conferir soma...).
xdotool mousemove --window "$main" 380 300 click 3
sleep 1
shot 4b-menu-do-download
# Its third item, "Conferir soma (SHA-256)…" (the menu opens at the click), then
# "Conferir" with no checksum: the window shows the file's SHA-256.
xdotool mousemove 480 381 click 1
if verify="$(find_window "^Conferir soma$")"; then
  xdotool windowfocus --sync "$verify" key Return
  sleep 3
  shot 4d-conferir-soma
  xdotool windowfocus --sync "$verify" key Escape
else
  echo "warning: Conferir soma did not open (menu position to calibrate)"
  xdotool key Escape
fi
xdotool windowfocus --sync "$main" key ctrl+f ctrl+a BackSpace Escape
sleep 0.5
sleep 0.5
xdotool windowfocus --sync "$main" key ctrl+comma
prefs="$(find_window "^Preferências$")"
sleep 1
shot 5-preferencias
xdotool windowfocus --sync "$prefs" key ctrl+Page_Down
sleep 1
shot 5b-filas
xdotool windowfocus --sync "$prefs" key ctrl+Page_Down
sleep 1
shot 5c-rede
xdotool windowfocus --sync "$prefs" key ctrl+Page_Down ctrl+Page_Down ctrl+Page_Down
sleep 1
shot 5f-aparencia
xdotool windowfocus --sync "$prefs" key Escape
for _ in $(seq 1 25); do xdotool search --name "^Preferências$" >/dev/null 2>&1 || break; sleep 0.2; done

# The light theme, as the settings change it (and back).
"$BIN/fisga" settings --theme claro >/dev/null
sleep 1.5
shot 4e-tema-claro
"$BIN/fisga" settings --theme escuro >/dev/null
sleep 0.5

echo "== batch download window"
xdotool windowfocus --sync "$main" key ctrl+shift+n
batch="$(find_window "^Adicionar em lote$")"
xdotool windowfocus --sync "$batch" type --delay 20 "$BASE/files/aula_*.bin"
sleep 1
shot 5d-lote
xdotool windowfocus --sync "$batch" key Escape
sleep 0.5

echo "== grabber window"
xdotool windowfocus --sync "$main" key ctrl+shift+g
grabber="$(find_window "^Baixar de um site")"
xdotool windowfocus --sync "$grabber" key ctrl+a
xdotool type --delay 20 "$BASE/site/"
xdotool key Return
sleep 2
shot 5e-grabber
xdotool windowfocus --sync "$grabber" key Escape

echo "== background mode (session autostart): no window, but it answers the browser"
pkill -f "$BIN/fisga-gui" || true
for _ in $(seq 1 25); do pgrep -f "$BIN/fisga-gui" >/dev/null || break; sleep 0.2; done
"$BIN/fisga-gui" --background &
sleep 3
if xdotool search --name "^Fisga$" >/dev/null 2>&1; then echo "main window opened"; exit 1; fi
host "{\"type\":\"download\",\"url\":\"$BASE/files/6291456.bin\",\"kind\":\"direct\"}"
win="$(find_window "^Novo download$")"
if pgrep -f "fisga-gui --pending" >/dev/null; then echo "a --pending GUI was launched"; exit 1; fi
sleep 1
shot 6-segundo-plano
xdotool windowfocus --sync "$win" key Escape
sleep 4.5
pgrep -f "fisga-gui --background" >/dev/null; echo "still running after the window closed"
grep -i "bandeja" "${XDG_STATE_HOME:-$HOME/.local/state}/fisga/fisga-gui.log" 2>/dev/null || true

"$BIN/fisga" list
ls -R "$HOME/Downloads"
echo "GUI SMOKE OK"
