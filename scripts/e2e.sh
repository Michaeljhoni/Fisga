#!/usr/bin/env bash
# End-to-end smoke test of fisgad + fisga + fisga-native-host in an isolated HOME,
# against a local test server (no internet needed).
# Usage: scripts/e2e.sh [bin-dir]   (default: target/debug; build with `cargo build --workspace --bins --examples`, so the dir also
# contains examples/range_server)
set -euo pipefail

BIN="$(cd "${1:-target/debug}" && pwd)"
SANDBOX="$(mktemp -d)"
export HOME="$SANDBOX/home" XDG_RUNTIME_DIR="$SANDBOX/run" XDG_DATA_HOME="$SANDBOX/data" XDG_CONFIG_HOME="$SANDBOX/home/.config"
# Stand-in for fisga-gui: starts fine but never connects, like a GUI without a display.
export FISGA_GUI=true
# Stand-in for `wl-paste --watch`: each line appended to this file is a copied text.
export FISGA_CLIPBOARD_CMD="tail -n 0 -F '$SANDBOX/clipboard'"
touch "$SANDBOX/clipboard"
# Scheduler: check the clock often, and record "suspend"/"poweroff" instead of running systemctl.
export FISGA_SCHEDULER_TICK_MS=500 FISGA_POWER_DELAY=1 FISGA_POWER_CMD="echo >> '$SANDBOX/power'"
# On complete: a stand-in ClamAV that finds a "virus" in 2621441.bin, and a sound that leaves a mark.
export FISGA_CLAMSCAN_CMD='f() { case "$1" in *2621441*) echo "$1: Eicar-Test FOUND"; exit 1;; esac; }; f'
export FISGA_SOUND_CMD="echo som >> '$SANDBOX/som'"
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

cleanup() { "$BIN/fisga" stop >/dev/null 2>&1 || true; kill "$SERVER_PID" 2>/dev/null || true; rm -rf "$SANDBOX"; }
trap cleanup EXIT

step() { printf '\n== %s\n' "$*"; }
# Native messaging framing: 32-bit little-endian length + JSON.
frame() {
  local n; n=$(printf %s "$1" | wc -c)  # bytes, not characters (UTF-8 locales)
  printf "\\x$(printf %02x $((n & 255)))\\x$(printf %02x $(((n >> 8) & 255)))\\x00\\x00%s" "$1"
}
host() { frame "$1" | "$BIN/fisga-native-host" | tail -c +5; echo; }
state_of() { "$BIN/fisga" list | awk -v id="$1" '$1 == id { print $2 }'; }
last_id() { "$BIN/fisga" list | awk '$1 ~ /^[0-9]+$/ && $1 > max { max = $1 } END { print max + 0 }'; }
wait_state() {
  for _ in $(seq 1 "${3:-120}"); do
    local state
    state="$(state_of "$1")"
    [ "$state" = "$2" ] && return 0
    if [ "$state" = erro ] && [ "$2" != erro ]; then
      echo "#$1 failed"; "$BIN/fisga" list; exit 1
    fi
    sleep 1
  done
  echo "timeout waiting for #$1 to be $2"; "$BIN/fisga" list; exit 1
}

# Settings from before the rename (LDM), in its old folder.
mkdir -p "$XDG_CONFIG_HOME/ldm"
echo '{"connections": 5}' > "$XDG_CONFIG_HOME/ldm/settings.json"

step "add + wait (daemon starts automatically)"
"$BIN/fisga" add "$SMALL" --wait

step "the old LDM folders moved to Fisga's"
test ! -e "$XDG_CONFIG_HOME/ldm" && test -f "$XDG_CONFIG_HOME/fisga/settings.json"
"$BIN/fisga" settings | grep "conexões por arquivo:    5"
"$BIN/fisga" settings --connections 8 >/dev/null
test "$(stat -c %s "$DL/10485760.bin")" -eq 10485760; echo "size ok"

step "pause / resume"
"$BIN/fisga" add "$LARGE" >/dev/null
sleep 2
"$BIN/fisga" pause 2
wait_state 2 pausado 20
ls "$DL" | grep -q '104857600.bin.fisga-part.json'; echo "control file kept"
# As if paused before the rename: the engine's files under LDM's names.
for f in "$DL"/104857600.bin.fisga-part*; do mv "$f" "${f/fisga-part/ldm-part}"; done
ls "$DL" | grep -q '104857600.bin.ldm-part.json'
"$BIN/fisga" list
"$BIN/fisga" resume 2
sleep 2
if ls "$DL" | grep -q 'ldm-part'; then echo "the .ldm-part files were not adopted"; exit 1; fi
wait_state 2 concluído 300
test "$(stat -c %s "$DL/104857600.bin")" -eq 104857600; echo "size ok after resume"
# Byte-for-byte check against a fresh download made in one go (32 connections).
"$BIN/fisga" get "$LARGE" -c 32 -o "$SANDBOX/reference" >/dev/null
cmp "$DL/104857600.bin" "$SANDBOX/reference/104857600.bin"; echo "content ok after pause/resume"

step "native host: ping"
host '{"type":"ping"}'

step "native host: download sent by the extension waits for confirmation"
reply="$(host "{\"type\":\"download\",\"url\":\"$SMALL\",\"kind\":\"direct\",\"referrer\":\"https://example.com/page\",\"title\":\"Teste\",\"userAgent\":\"Mozilla/5.0 (X11; Linux x86_64)\",\"cookies\":[{\"name\":\"session\",\"value\":\"abc\",\"domain\":\"127.0.0.1\",\"path\":\"/\",\"secure\":true,\"hostOnly\":true}]}")"
echo "$reply"
echo "$reply" | grep -q '"type":"added"'
echo "$reply" | grep -q '"pending":true'
wait_state 3 aguardando 5
"$BIN/fisga" list
# What the "Baixar agora" button sends, with another name and folder.
"$BIN/fisga" confirm 3 --name confirmado.bin -o "$SANDBOX/escolhida"
wait_state 3 concluído 120
test "$(stat -c %s "$SANDBOX/escolhida/confirmado.bin")" -eq 10485760; echo "confirmed download ok"

step "native host: \"Cancelar\" in the confirmation window"
host "{\"type\":\"download\",\"url\":\"$SMALL\",\"kind\":\"direct\"}" | grep -q '"pending":true'
wait_state 4 aguardando 5
"$BIN/fisga" rm 4
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
"$BIN/fisga" add "$OWN" --wait >/dev/null
host "{\"type\":\"download\",\"url\":\"$OWN\",\"kind\":\"direct\"}" | grep -q '"pending":true'
id="$(last_id)"; wait_state "$id" aguardando 5
# A direct link has no quality to choose.
if "$BIN/fisga" confirm "$id" --quality 720 2>"$SANDBOX/err"; then echo "quality accepted"; exit 1; fi
grep "opções de qualidade" "$SANDBOX/err"
"$BIN/fisga" confirm "$id" --if-exists rename
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/3145728 (1).bin")" -eq 3145728; echo "saved as '3145728 (1).bin'"
inode="$(stat -c %i "$DL/3145728.bin")"
host "{\"type\":\"download\",\"url\":\"$OWN\",\"kind\":\"direct\"}" | grep -q '"pending":true'
id="$(last_id)"; wait_state "$id" aguardando 5
"$BIN/fisga" confirm "$id" --if-exists overwrite
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
"$BIN/fisga" confirm "$id"
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/1048576.bin")" -eq 1048576; echo "copied link downloaded"
"$BIN/fisga" settings --watch-clipboard false | grep "capturar links copiados: não"
echo "$BASE/files/2097152.bin" >> "$SANDBOX/clipboard"
sleep 1
test "$(last_id)" -eq "$id"; echo "turned off: ignored"
"$BIN/fisga" settings --watch-clipboard true >/dev/null

step "scheduler: outside its hours the queue waits; inside, it runs, then suspends"
"$BIN/fisga" schedule --on --start "$(date -d '+2 hours' +%H:%M)" --stop "$(date -d '+3 hours' +%H:%M)" --when-done suspender
"$BIN/fisga" settings | grep "agendador:"
"$BIN/fisga" add "$BASE/files/5242880.bin" >/dev/null
id="$(last_id)"
sleep 2
test "$(state_of "$id")" = na; echo "#$id waits in the queue"
if "$BIN/fisga" schedule --cancel 2>/dev/null; then echo "cancelled a plan that does not exist"; exit 1; fi
# Now inside the window: the queue starts, finishes, and the computer would be suspended.
"$BIN/fisga" schedule --start "$(date -d '-1 minute' +%H:%M)" --stop "$(date -d '+30 minutes' +%H:%M)"
wait_state "$id" concluído 60
for _ in $(seq 1 40); do [ -s "$SANDBOX/power" ] && break; sleep 0.5; done
test "$(cat "$SANDBOX/power")" = suspend; echo "queue done: suspend requested"
"$BIN/fisga" schedule --days seg,qua,sex | grep "seg, qua, sex"
"$BIN/fisga" schedule --days todos --off | grep "desligado"

step "queues: one at a time in a second queue, its order, and removing it"
"$BIN/fisga" settings --speed-limit 4096 >/dev/null   # 4 MiB/s: slow enough to watch the queue
"$BIN/fisga" queue add Noite --max-parallel 1 | grep "Noite"
q="$("$BIN/fisga" queue list | awk '$2 == "Noite" { print $1 }')"
"$BIN/fisga" add "$BASE/files/20971520.bin" -Q "$q" >/dev/null; a="$(last_id)"
"$BIN/fisga" add "$BASE/files/15728640.bin" -Q "$q" >/dev/null; b="$(last_id)"
"$BIN/fisga" add "$BASE/files/12582912.bin" -Q "$q" >/dev/null; c="$(last_id)"
sleep 1
"$BIN/fisga" list | grep "\[Noite\]"
test "$(state_of "$a")" = baixando; test "$(state_of "$b")" = na; test "$(state_of "$c")" = na
echo "one at a time in queue #$q"
"$BIN/fisga" move "$c" --top
wait_state "$a" concluído 60
sleep 1
test "$(state_of "$c")" = baixando; test "$(state_of "$b")" = na; echo "moved to the top: #$c went before #$b"
"$BIN/fisga" queue rm "$q" >/dev/null
if "$BIN/fisga" queue list | grep -q Noite; then echo "queue still there"; exit 1; fi
# #b is back in the main queue, which has free slots.
wait_state "$b" baixando 10
wait_state "$c" concluído 60
wait_state "$b" concluído 60

step "refresh address: a new link to the same file continues the download"
# "?token=..." is another link to the same file (the test server ignores it).
"$BIN/fisga" add "$BASE/files/31457280.bin?token=antigo" >/dev/null; id="$(last_id)"
sleep 2
"$BIN/fisga" pause "$id"; wait_state "$id" pausado 20
if "$BIN/fisga" refresh "$id" "$BASE/files/7340032.bin" 2>"$SANDBOX/err"; then echo "accepted another file"; exit 1; fi
grep "outro arquivo" "$SANDBOX/err"
"$BIN/fisga" refresh "$id" "$BASE/files/31457280.bin?token=novo"
wait_state "$id" concluído 60
"$BIN/fisga" get "$BASE/files/31457280.bin" -o "$SANDBOX/reference" >/dev/null
cmp "$DL/31457280.bin" "$SANDBOX/reference/31457280.bin"; echo "content ok after the new link"

step "refresh address: caught from the browser"
"$BIN/fisga" add "$BASE/files/26214400.bin?token=velho" >/dev/null; id="$(last_id)"
sleep 2
"$BIN/fisga" pause "$id"; wait_state "$id" pausado 20
"$BIN/fisga" refresh "$id" --capture
"$BIN/fisga" list | grep "aguardando novo endereço"
host "{\"type\":\"download\",\"url\":\"$BASE/files/26214400.bin?token=fresco\",\"kind\":\"direct\"}" | grep -q '"pending":true'
wait_state "$id" concluído 60
test -z "$(state_of $((id + 1)))"; echo "the browser's download went to #$id instead of a new one"
"$BIN/fisga" get "$BASE/files/26214400.bin" -o "$SANDBOX/reference" >/dev/null
cmp "$DL/26214400.bin" "$SANDBOX/reference/26214400.bin"; echo "content ok"
"$BIN/fisga" settings --speed-limit 0 >/dev/null

step "site login: a password-protected file"
"$BIN/fisga" add "$BASE/private/files/9437184.bin" >/dev/null; id="$(last_id)"
wait_state "$id" erro 30
"$BIN/fisga" list | grep "401"
echo "senha" | "$BIN/fisga" login add 127.0.0.1 teste
"$BIN/fisga" login | grep "127.0.0.1 .* teste"
if grep -q senha "$XDG_CONFIG_HOME/fisga/settings.json" "$HOME/.local/state/fisga/fisgad.log" 2>/dev/null; then
  echo "the password leaked into the settings or the log"; exit 1
fi
test "$(stat -c %a "$XDG_CONFIG_HOME/fisga/logins.json")" = 600; echo "logins.json is private"
"$BIN/fisga" resume "$id"
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/9437184.bin")" -eq 9437184; echo "downloaded with the site login"
"$BIN/fisga" login rm 127.0.0.1 | grep "nenhum login"

step "proxy: downloads go through it"
# The test server is also a proxy; this host does not exist, so only the proxy can serve it.
"$BIN/fisga" settings --proxy "$BASE" | grep "proxy:.*127.0.0.1"
"$BIN/fisga" add "http://fisga-proxy-test.invalid/files/8388608.bin" >/dev/null; id="$(last_id)"
wait_state "$id" concluído 60
test "$(stat -c %s "$DL/8388608.bin")" -eq 8388608; echo "downloaded through the proxy"
if "$BIN/fisga" settings --proxy "ftp://proxy:21" 2>"$SANDBOX/err"; then echo "accepted an ftp proxy"; exit 1; fi
grep "não suportado" "$SANDBOX/err"
"$BIN/fisga" settings --proxy sistema | grep "proxy:.*sistema"

step "browser settings: file types and force key reach the extension"
host '{"type":"settings"}' | tee "$SANDBOX/browser" | grep '"forceKey":"alt"'
grep -q '"safetensors"' "$SANDBOX/browser"
"$BIN/fisga" settings --file-types ".ZIP, *.gguf zip" --force-key ctrl+alt | grep "tecla do navegador:      Ctrl+Alt"
host '{"type":"settings"}' | grep '"fileTypes":\["zip","gguf"\],"forceKey":"ctrl_alt"'
if "$BIN/fisga" settings --force-key super 2>"$SANDBOX/err"; then echo "accepted a bad key"; exit 1; fi
"$BIN/fisga" settings --file-types padrao --force-key alt | grep "tipos do navegador:.*safetensors"

step "native host: invalid message"
host '{"type":"nope"}'

step "install-hosts"
mkdir -p "$XDG_CONFIG_HOME/google-chrome" "$XDG_CONFIG_HOME/BraveSoftware/Brave-Browser"
"$BIN/fisga" install-hosts
cat "$XDG_CONFIG_HOME/google-chrome/NativeMessagingHosts/io.fisga.native.json"
cat "$HOME/.mozilla/native-messaging-hosts/io.fisga.native.json"

step "settings: no category subfolders"
"$BIN/fisga" settings --category-folders false
"$BIN/fisga" add "$SMALL" --wait >/dev/null
test "$(stat -c %s "$HOME/Downloads/10485760.bin")" -eq 10485760; echo "saved in the base folder"
"$BIN/fisga" settings

step "settings: speed limit (1 MiB/s for all downloads)"
"$BIN/fisga" settings --speed-limit 1024 | grep "limite de velocidade"
start=$(date +%s.%N)
"$BIN/fisga" add "$BASE/files/4194304.bin" --wait >/dev/null
elapsed=$(echo "$(date +%s.%N) $start" | awk '{ printf "%.1f", $1 - $2 }')
echo "4 MiB took ${elapsed}s"
awk -v t="$elapsed" 'BEGIN { exit !(t >= 3.0) }'; echo "limit respected"
"$BIN/fisga" settings --speed-limit 0 >/dev/null

step "speed limit of one download (2 MiB/s), on top of the global one"
"$BIN/fisga" add "$BASE/files/16252928.bin" >/dev/null; id="$(last_id)"
"$BIN/fisga" pause "$id"; wait_state "$id" pausado 20
"$BIN/fisga" limit "$id" 2048 | grep "no máximo"
"$BIN/fisga" list | grep "máx. 2.00 MiB/s"
done_before=$("$BIN/fisga" list | awk -v id="$id" '$1 == id { print $3 }')
echo "already downloaded before the limit: ${done_before}%"
start=$(date +%s.%N)
"$BIN/fisga" resume "$id"
wait_state "$id" concluído 60
elapsed=$(echo "$(date +%s.%N) $start" | awk '{ printf "%.1f", $1 - $2 }')
echo "the rest of 15.5 MiB took ${elapsed}s"
# Over 10 MiB were left: at 2 MiB/s that takes 5 s or more.
awk -v t="$elapsed" 'BEGIN { exit !(t >= 3.0) }'; echo "own limit respected"
test "$(stat -c %s "$HOME/Downloads/16252928.bin")" -eq 16252928
"$BIN/fisga" limit "$id" 0 | grep "sem limite"

step "batch download: a pattern and a list"
test "$("$BIN/fisga" batch "$BASE/files/13107*.bin" --from 20 --to 22 --dry-run | wc -l)" -eq 3
if "$BIN/fisga" batch "$BASE/files/x.bin" --from 1 --to 2 2>"$SANDBOX/err"; then echo "accepted a pattern without *"; exit 1; fi
grep "exatamente um" "$SANDBOX/err"
"$BIN/fisga" batch "$BASE/files/13107*.bin" --from 20 --to 22 -o "$SANDBOX/lote" | grep "3 downloads adicionados"
last="$(last_id)"
for id in $((last - 2)) $((last - 1)) "$last"; do wait_state "$id" concluído 60; done
for size in 1310720 1310721 1310722; do test "$(stat -c %s "$SANDBOX/lote/$size.bin")" -eq "$size"; done
echo "the pattern's 3 files are there"
printf '%s\n' "veja: $BASE/files/1638401.bin" "texto solto" "<$BASE/files/1638402.bin>" "$BASE/files/1638401.bin" \
  | "$BIN/fisga" batch --file - --paused -o "$SANDBOX/lote" | grep "2 downloads adicionados"
last="$(last_id)"
test "$(state_of "$last")" = pausado; test "$(state_of $((last - 1)))" = pausado; echo "the list's 2 links, paused"
"$BIN/fisga" export --unfinished | grep -c "1638401.bin\|1638402.bin" | grep -qx 2; echo "export lists the unfinished ones"
"$BIN/fisga" export > "$SANDBOX/exportado.txt"
test "$("$BIN/fisga" batch --file "$SANDBOX/exportado.txt" --dry-run | wc -l)" -eq "$(sort -u "$SANDBOX/exportado.txt" | wc -l)"
echo "the exported list reads back as a batch"
"$BIN/fisga" rm "$last"; "$BIN/fisga" rm $((last - 1))

step "grabber: the files of a page and of the pages below it"
test "$("$BIN/fisga" grab "$BASE/site/" --depth 0 | wc -l)" -eq 2; echo "depth 0: the page's 2 files"
"$BIN/fisga" grab "$BASE/site/" --depth 1 --pattern '*.bin' > "$SANDBOX/grab.txt"
test "$(wc -l < "$SANDBOX/grab.txt")" -eq 2; grep -q 1507328.bin "$SANDBOX/grab.txt"; echo "depth 1: the subfolder's file too, the repeated one once"
"$BIN/fisga" grab "$BASE/site/" --type documento | grep -q "apostila.pdf"; echo "type filter"
"$BIN/fisga" grab "$BASE/site/" --depth 1 --pattern '*.bin' --add -o "$SANDBOX/site" | grep "2 downloads adicionados"
last="$(last_id)"
wait_state "$last" concluído 60; wait_state $((last - 1)) concluído 60
test "$(stat -c %s "$SANDBOX/site/1507328.bin")" -eq 1507328; echo "grabbed files downloaded"

step "on complete: antivirus, the user's command, the sound"
"$BIN/fisga" settings --antivirus true --on-complete "printf '%s\n' \"\$FISGA_FILE\" >> '$SANDBOX/feito'" | grep "antivírus:  sim"
rm -f "$SANDBOX/som"
"$BIN/fisga" add "$BASE/files/1966080.bin" --wait >/dev/null
for _ in $(seq 1 30); do [ -s "$SANDBOX/som" ] && break; sleep 0.2; done
grep -qx "$HOME/Downloads/1966080.bin" "$SANDBOX/feito"; grep -q som "$SANDBOX/som"; echo "clean file: command got its path, sound played"
"$BIN/fisga" add "$BASE/files/2621441.bin" --wait >/dev/null
id="$(last_id)"
for _ in $(seq 1 30); do "$BIN/fisga" list | grep -q "ClamAV encontrou" && break; sleep 0.2; done
"$BIN/fisga" list | grep "ClamAV encontrou Eicar-Test"
if grep -q 2621441 "$SANDBOX/feito"; then echo "the command ran on an infected file"; exit 1; fi
echo "infected file flagged, command skipped"
"$BIN/fisga" settings --antivirus false --on-complete "" | grep "comando:    nenhum"

step "checksum: the file is checked against the one the page published"
sum="$(sha256sum "$DL/10485760.bin" | cut -d' ' -f1)"
"$BIN/fisga" add "$SMALL" -o "$SANDBOX/soma" --checksum "SHA-256: ${sum^^}" --wait | grep "✓"
"$BIN/fisga" list | grep "soma conferida"; echo "right checksum: checked"
if "$BIN/fisga" add "$SMALL" -o "$SANDBOX/soma-errada" --checksum "sha256:$(printf '0%.0s' $(seq 64))" --wait 2> "$SANDBOX/soma.err"; then
  echo "a wrong checksum was accepted"; exit 1
fi
grep "não confere" "$SANDBOX/soma.err"
"$BIN/fisga" list | grep "SHA-256 não confere"; echo "wrong checksum: flagged"
id="$(last_id)"
# The right one again, with the checksum it keeps.
"$BIN/fisga" verify $((id - 1)) | grep -x "SHA-256: $sum"; echo "verify again: the file's SHA-256"
# The wrong one with another checksum, as a SHA1SUMS file has it.
(cd "$SANDBOX/soma-errada" && sha1sum 10485760.bin) > "$SANDBOX/SHA1SUMS"
"$BIN/fisga" verify "$id" "$(cat "$SANDBOX/SHA1SUMS")" | grep "confere com a soma da página"; echo "verify with a SHA1SUMS line"
if "$BIN/fisga" add "$SMALL" --checksum "md5:098f6bcd4621d373cade4e832627b4f6" 2>/dev/null; then
  echo "an MD5 was accepted"; exit 1
fi
echo "invalid checksum refused"

step "list"
"$BIN/fisga" list
echo; echo "E2E OK"
