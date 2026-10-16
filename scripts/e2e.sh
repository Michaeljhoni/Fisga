#!/usr/bin/env bash
# End-to-end smoke test of ldmd + ldm + ldm-native-host in an isolated HOME,
# against a local test server (no internet needed).
# Usage: scripts/e2e.sh [bin-dir]   (default: target/debug; build with `cargo build --workspace --bins --examples`, so the dir also
# contains examples/range_server)
set -euo pipefail

BIN="$(cd "${1:-target/debug}" && pwd)"
SANDBOX="$(mktemp -d)"
export HOME="$SANDBOX/home" XDG_RUNTIME_DIR="$SANDBOX/run" XDG_DATA_HOME="$SANDBOX/data" XDG_CONFIG_HOME="$SANDBOX/home/.config"
# Stand-in for ldm-gui: starts fine but never connects, like a GUI without a display.
export LDM_GUI=true
# Stand-in for `wl-paste --watch`: each line appended to this file is a copied text.
export LDM_CLIPBOARD_CMD="tail -n 0 -F '$SANDBOX/clipboard'"
touch "$SANDBOX/clipboard"
mkdir -p "$HOME/Downloads" "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME"
# .bin files belong to the "Programas" category, saved in its own subfolder.
DL="$HOME/Downloads/Programas"
chmod 700 "$XDG_RUNTIME_DIR"

# 16 KiB every 10 ms per connection (~1.6 MB/s each): slow enough to pause mid-download.
"$BIN/examples/range_server" 0 10 > "$SANDBOX/server.url" &
SERVER_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/server.url" ] && break; sleep 0.1; done
BASE="$(cat "$SANDBOX/server.url")"
SMALL="$BASE/files/10485760.bin"
LARGE="$BASE/files/104857600.bin"

cleanup() { "$BIN/ldm" stop >/dev/null 2>&1 || true; kill "$SERVER_PID" 2>/dev/null || true; rm -rf "$SANDBOX"; }
trap cleanup EXIT

step() { printf '\n== %s\n' "$*"; }
# Native messaging framing: 32-bit little-endian length + JSON.
frame() {
  local n; n=$(printf %s "$1" | wc -c)  # bytes, not characters (UTF-8 locales)
  printf "\\x$(printf %02x $((n & 255)))\\x$(printf %02x $(((n >> 8) & 255)))\\x00\\x00%s" "$1"
}
host() { frame "$1" | "$BIN/ldm-native-host" | tail -c +5; echo; }
state_of() { "$BIN/ldm" list | awk -v id="$1" '$1 == id { print $2 }'; }
last_id() { "$BIN/ldm" list | awk '$1 ~ /^[0-9]+$/ && $1 > max { max = $1 } END { print max + 0 }'; }
wait_state() {
  for _ in $(seq 1 "${3:-120}"); do
    local state
    state="$(state_of "$1")"
    [ "$state" = "$2" ] && return 0
    if [ "$state" = erro ] && [ "$2" != erro ]; then
      echo "#$1 failed"; "$BIN/ldm" list; exit 1
    fi
    sleep 1
  done
  echo "timeout waiting for #$1 to be $2"; "$BIN/ldm" list; exit 1
}

step "add + wait (daemon starts automatically)"
"$BIN/ldm" add "$SMALL" --wait
test "$(stat -c %s "$DL/10485760.bin")" -eq 10485760; echo "size ok"

step "pause / resume"
"$BIN/ldm" add "$LARGE" >/dev/null
sleep 2
"$BIN/ldm" pause 2
wait_state 2 pausado 20
ls "$DL" | grep -q '104857600.bin.ldm-part.json'; echo "control file kept"
"$BIN/ldm" list
"$BIN/ldm" resume 2
wait_state 2 concluído 300
test "$(stat -c %s "$DL/104857600.bin")" -eq 104857600; echo "size ok after resume"
# Byte-for-byte check against a fresh download made in one go (32 connections).
"$BIN/ldm" get "$LARGE" -c 32 -o "$SANDBOX/reference" >/dev/null
cmp "$DL/104857600.bin" "$SANDBOX/reference/104857600.bin"; echo "content ok after pause/resume"

step "native host: ping"
host '{"type":"ping"}'

step "native host: download sent by the extension waits for confirmation"
reply="$(host "{\"type\":\"download\",\"url\":\"$SMALL\",\"kind\":\"direct\",\"referrer\":\"https://example.com/page\",\"title\":\"Teste\",\"userAgent\":\"Mozilla/5.0 (X11; Linux x86_64)\",\"cookies\":[{\"name\":\"session\",\"value\":\"abc\",\"domain\":\"127.0.0.1\",\"path\":\"/\",\"secure\":true,\"hostOnly\":true}]}")"
echo "$reply"
echo "$reply" | grep -q '"type":"added"'
echo "$reply" | grep -q '"pending":true'
wait_state 3 aguardando 5
"$BIN/ldm" list
# What the "Baixar agora" button sends, with another name and folder.
"$BIN/ldm" confirm 3 --name confirmado.bin -o "$SANDBOX/escolhida"
wait_state 3 concluído 120
test "$(stat -c %s "$SANDBOX/escolhida/confirmado.bin")" -eq 10485760; echo "confirmed download ok"

step "native host: \"Cancelar\" in the confirmation window"
host "{\"type\":\"download\",\"url\":\"$SMALL\",\"kind\":\"direct\"}" | grep -q '"pending":true'
wait_state 4 aguardando 5
"$BIN/ldm" rm 4
test -z "$(state_of 4)"; echo "cancelled download removed"

step "native host: extension option to start without confirmation"
reply="$(host "{\"type\":\"download\",\"url\":\"$SMALL\",\"kind\":\"direct\",\"confirm\":false}")"
echo "$reply"
echo "$reply" | grep -q '"pending":false'
wait_state 5 concluído 120

step "no GUI connects: the download is queued anyway after 15 s"
host "{\"type\":\"download\",\"url\":\"$SMALL\",\"kind\":\"direct\"}" | grep -q '"pending":true'
wait_state 6 aguardando 5
wait_state 6 concluído 60
ls -la "$HOME/Downloads"

step "file already exists: another name, or replace it"
OWN="$BASE/files/3145728.bin"
"$BIN/ldm" add "$OWN" --wait >/dev/null
host "{\"type\":\"download\",\"url\":\"$OWN\",\"kind\":\"direct\"}" | grep -q '"pending":true'
id="$(last_id)"; wait_state "$id" aguardando 5
# A direct link has no quality to choose.
if "$BIN/ldm" confirm "$id" --quality 720 2>"$SANDBOX/err"; then echo "quality accepted"; exit 1; fi
grep "opções de qualidade" "$SANDBOX/err"
"$BIN/ldm" confirm "$id" --if-exists rename
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/3145728 (1).bin")" -eq 3145728; echo "saved as '3145728 (1).bin'"
inode="$(stat -c %i "$DL/3145728.bin")"
host "{\"type\":\"download\",\"url\":\"$OWN\",\"kind\":\"direct\"}" | grep -q '"pending":true'
id="$(last_id)"; wait_state "$id" aguardando 5
"$BIN/ldm" confirm "$id" --if-exists overwrite
wait_state "$id" concluído 60
test "$(stat -c %i "$DL/3145728.bin")" != "$inode"; test ! -e "$DL/3145728 (2).bin"
test "$(stat -c %s "$DL/3145728.bin")" -eq 3145728; echo "existing file replaced"

step "clipboard: a copied link to a file asks to download it"
before="$(last_id)"
echo "https://example.com/pagina.html" >> "$SANDBOX/clipboard"
echo "texto qualquer" >> "$SANDBOX/clipboard"
echo "$BASE/files/1048576.bin" >> "$SANDBOX/clipboard"
for _ in $(seq 1 30); do [ "$(last_id)" != "$before" ] && break; sleep 0.2; done
id="$(last_id)"
test "$id" -eq $((before + 1)); echo "only the file link was caught (#$id)"
wait_state "$id" aguardando 5
"$BIN/ldm" confirm "$id"
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/1048576.bin")" -eq 1048576; echo "copied link downloaded"
"$BIN/ldm" settings --watch-clipboard false | grep "capturar links copiados: não"
echo "$BASE/files/2097152.bin" >> "$SANDBOX/clipboard"
sleep 1
test "$(last_id)" -eq "$id"; echo "turned off: ignored"
"$BIN/ldm" settings --watch-clipboard true >/dev/null

step "native host: invalid message"
host '{"type":"nope"}'

step "install-hosts"
mkdir -p "$XDG_CONFIG_HOME/google-chrome" "$XDG_CONFIG_HOME/BraveSoftware/Brave-Browser"
"$BIN/ldm" install-hosts
cat "$XDG_CONFIG_HOME/google-chrome/NativeMessagingHosts/io.ldm.native.json"
cat "$HOME/.mozilla/native-messaging-hosts/io.ldm.native.json"

step "settings: no category subfolders"
"$BIN/ldm" settings --category-folders false
"$BIN/ldm" add "$SMALL" --wait >/dev/null
test "$(stat -c %s "$HOME/Downloads/10485760.bin")" -eq 10485760; echo "saved in the base folder"
"$BIN/ldm" settings

step "settings: speed limit (1 MiB/s for all downloads)"
"$BIN/ldm" settings --speed-limit 1024 | grep "limite de velocidade"
start=$(date +%s.%N)
"$BIN/ldm" add "$BASE/files/4194304.bin" --wait >/dev/null
elapsed=$(echo "$(date +%s.%N) $start" | awk '{ printf "%.1f", $1 - $2 }')
echo "4 MiB took ${elapsed}s"
awk -v t="$elapsed" 'BEGIN { exit !(t >= 3.0) }'; echo "limit respected"
"$BIN/ldm" settings --speed-limit 0 >/dev/null

step "list"
"$BIN/ldm" list
echo; echo "E2E OK"
