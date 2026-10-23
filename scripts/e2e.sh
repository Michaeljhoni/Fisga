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
# Scheduler: check the clock often, and record "suspend"/"poweroff" instead of running systemctl.
export LDM_SCHEDULER_TICK_MS=500 LDM_POWER_DELAY=1 LDM_POWER_CMD="echo >> '$SANDBOX/power'"
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

step "scheduler: outside its hours the queue waits; inside, it runs, then suspends"
"$BIN/ldm" schedule --on --start "$(date -d '+2 hours' +%H:%M)" --stop "$(date -d '+3 hours' +%H:%M)" --when-done suspender
"$BIN/ldm" settings | grep "agendador:"
"$BIN/ldm" add "$BASE/files/5242880.bin" >/dev/null
id="$(last_id)"
sleep 2
test "$(state_of "$id")" = na; echo "#$id waits in the queue"
if "$BIN/ldm" schedule --cancel 2>/dev/null; then echo "cancelled a plan that does not exist"; exit 1; fi
# Now inside the window: the queue starts, finishes, and the computer would be suspended.
"$BIN/ldm" schedule --start "$(date -d '-1 minute' +%H:%M)" --stop "$(date -d '+30 minutes' +%H:%M)"
wait_state "$id" concluído 60
for _ in $(seq 1 40); do [ -s "$SANDBOX/power" ] && break; sleep 0.5; done
test "$(cat "$SANDBOX/power")" = suspend; echo "queue done: suspend requested"
"$BIN/ldm" schedule --off | grep "desligado"

step "queues: one at a time in a second queue, its order, and removing it"
"$BIN/ldm" settings --speed-limit 4096 >/dev/null   # 4 MiB/s: slow enough to watch the queue
"$BIN/ldm" queue add Noite --max-parallel 1 | grep "Noite"
q="$("$BIN/ldm" queue list | awk '$2 == "Noite" { print $1 }')"
"$BIN/ldm" add "$BASE/files/20971520.bin" -Q "$q" >/dev/null; a="$(last_id)"
"$BIN/ldm" add "$BASE/files/15728640.bin" -Q "$q" >/dev/null; b="$(last_id)"
"$BIN/ldm" add "$BASE/files/12582912.bin" -Q "$q" >/dev/null; c="$(last_id)"
sleep 1
"$BIN/ldm" list | grep "\[Noite\]"
test "$(state_of "$a")" = baixando; test "$(state_of "$b")" = na; test "$(state_of "$c")" = na
echo "one at a time in queue #$q"
"$BIN/ldm" move "$c" --top
wait_state "$a" concluído 60
sleep 1
test "$(state_of "$c")" = baixando; test "$(state_of "$b")" = na; echo "moved to the top: #$c went before #$b"
"$BIN/ldm" queue rm "$q" >/dev/null
if "$BIN/ldm" queue list | grep -q Noite; then echo "queue still there"; exit 1; fi
# #b is back in the main queue, which has free slots.
wait_state "$b" baixando 10
wait_state "$c" concluído 60
wait_state "$b" concluído 60

step "refresh address: a new link to the same file continues the download"
# "?token=..." is another link to the same file (the test server ignores it).
"$BIN/ldm" add "$BASE/files/31457280.bin?token=antigo" >/dev/null; id="$(last_id)"
sleep 2
"$BIN/ldm" pause "$id"; wait_state "$id" pausado 20
if "$BIN/ldm" refresh "$id" "$BASE/files/7340032.bin" 2>"$SANDBOX/err"; then echo "accepted another file"; exit 1; fi
grep "outro arquivo" "$SANDBOX/err"
"$BIN/ldm" refresh "$id" "$BASE/files/31457280.bin?token=novo"
wait_state "$id" concluído 60
"$BIN/ldm" get "$BASE/files/31457280.bin" -o "$SANDBOX/reference" >/dev/null
cmp "$DL/31457280.bin" "$SANDBOX/reference/31457280.bin"; echo "content ok after the new link"

step "refresh address: caught from the browser"
"$BIN/ldm" add "$BASE/files/26214400.bin?token=velho" >/dev/null; id="$(last_id)"
sleep 2
"$BIN/ldm" pause "$id"; wait_state "$id" pausado 20
"$BIN/ldm" refresh "$id" --capture
"$BIN/ldm" list | grep "aguardando novo endereço"
host "{\"type\":\"download\",\"url\":\"$BASE/files/26214400.bin?token=fresco\",\"kind\":\"direct\"}" | grep -q '"pending":true'
wait_state "$id" concluído 60
test -z "$(state_of $((id + 1)))"; echo "the browser's download went to #$id instead of a new one"
"$BIN/ldm" get "$BASE/files/26214400.bin" -o "$SANDBOX/reference" >/dev/null
cmp "$DL/26214400.bin" "$SANDBOX/reference/26214400.bin"; echo "content ok"
"$BIN/ldm" settings --speed-limit 0 >/dev/null

step "site login: a password-protected file"
"$BIN/ldm" add "$BASE/private/files/9437184.bin" >/dev/null; id="$(last_id)"
wait_state "$id" erro 30
"$BIN/ldm" list | grep "401"
echo "senha" | "$BIN/ldm" login add 127.0.0.1 teste
"$BIN/ldm" login | grep "127.0.0.1 .* teste"
if grep -q senha "$XDG_CONFIG_HOME/ldm/settings.json" "$HOME/.local/state/ldm/ldmd.log" 2>/dev/null; then
  echo "the password leaked into the settings or the log"; exit 1
fi
test "$(stat -c %a "$XDG_CONFIG_HOME/ldm/logins.json")" = 600; echo "logins.json is private"
"$BIN/ldm" resume "$id"
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/9437184.bin")" -eq 9437184; echo "downloaded with the site login"
"$BIN/ldm" login rm 127.0.0.1 | grep "nenhum login"

step "proxy: downloads go through it"
# The test server is also a proxy; this host does not exist, so only the proxy can serve it.
"$BIN/ldm" settings --proxy "$BASE" | grep "proxy:.*127.0.0.1"
"$BIN/ldm" add "http://ldm-proxy-test.invalid/files/8388608.bin" >/dev/null; id="$(last_id)"
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/8388608.bin")" -eq 8388608; echo "downloaded through the proxy"
if "$BIN/ldm" settings --proxy "ftp://proxy:21" 2>"$SANDBOX/err"; then echo "accepted an ftp proxy"; exit 1; fi
grep "não suportado" "$SANDBOX/err"
"$BIN/ldm" settings --proxy sistema | grep "proxy:.*sistema"

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

step "speed limit of one download (2 MiB/s), on top of the global one"
"$BIN/ldm" add "$BASE/files/16252928.bin" >/dev/null; id="$(last_id)"
"$BIN/ldm" pause "$id"; wait_state "$id" pausado 20
"$BIN/ldm" limit "$id" 2048 | grep "no máximo"
"$BIN/ldm" list | grep "máx. 2.00 MiB/s"
done_before=$("$BIN/ldm" list | awk -v id="$id" '$1 == id { print $3 }')
echo "already downloaded before the limit: ${done_before}%"
start=$(date +%s.%N)
"$BIN/ldm" resume "$id"
wait_state "$id" concluído 60
elapsed=$(echo "$(date +%s.%N) $start" | awk '{ printf "%.1f", $1 - $2 }')
echo "the rest of 15.5 MiB took ${elapsed}s"
# Over 10 MiB were left: at 2 MiB/s that takes 5 s or more.
awk -v t="$elapsed" 'BEGIN { exit !(t >= 3.0) }'; echo "own limit respected"
test "$(stat -c %s "$HOME/Downloads/16252928.bin")" -eq 16252928
"$BIN/ldm" limit "$id" 0 | grep "sem limite"

step "batch download: a pattern and a list"
test "$("$BIN/ldm" batch "$BASE/files/13107*.bin" --from 20 --to 22 --dry-run | wc -l)" -eq 3
if "$BIN/ldm" batch "$BASE/files/x.bin" --from 1 --to 2 2>"$SANDBOX/err"; then echo "accepted a pattern without *"; exit 1; fi
grep "exatamente um" "$SANDBOX/err"
"$BIN/ldm" batch "$BASE/files/13107*.bin" --from 20 --to 22 -o "$SANDBOX/lote" | grep "3 downloads adicionados"
last="$(last_id)"
for id in $((last - 2)) $((last - 1)) "$last"; do wait_state "$id" concluído 60; done
for size in 1310720 1310721 1310722; do test "$(stat -c %s "$SANDBOX/lote/$size.bin")" -eq "$size"; done
echo "the pattern's 3 files are there"
printf '%s\n' "veja: $BASE/files/1638401.bin" "texto solto" "<$BASE/files/1638402.bin>" "$BASE/files/1638401.bin" \
  | "$BIN/ldm" batch --file - --paused -o "$SANDBOX/lote" | grep "2 downloads adicionados"
last="$(last_id)"
test "$(state_of "$last")" = pausado; test "$(state_of $((last - 1)))" = pausado; echo "the list's 2 links, paused"
"$BIN/ldm" export --unfinished | grep -c "1638401.bin\|1638402.bin" | grep -qx 2; echo "export lists the unfinished ones"
"$BIN/ldm" export > "$SANDBOX/exportado.txt"
test "$("$BIN/ldm" batch --file "$SANDBOX/exportado.txt" --dry-run | wc -l)" -eq "$(sort -u "$SANDBOX/exportado.txt" | wc -l)"
echo "the exported list reads back as a batch"
"$BIN/ldm" rm "$last"; "$BIN/ldm" rm $((last - 1))

step "list"
"$BIN/ldm" list
echo; echo "E2E OK"
