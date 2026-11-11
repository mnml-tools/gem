# Scenario: downloads through the context menu into the default download folder (the XDG
# Downloads folder of the run), byte-exact, with the rows done in the Transfers panel.
. "$ROOT/scripts/e2e/lib.sh"

# download <name>: right-click the first row (filtered to <name>) → Download.
download() {
  filter_to "$1"
  rclick 60 "$FIRST_ROW_Y"
  sleep 0.6
  click 116 $((FIRST_ROW_Y + 74))   # "Download", the third menu item
}

init_db
seed_site 01e2e000-0000-7000-8000-000000000001 "docker sftp" sftp 2222
trust_sftp
start_gem
connect_site 0
goto /home/gem/binary
download bytes.bin
shot 01-context-menu-done
download pixel.png
goto /home/gem/samples
download main.go
wait_until 20 test -f "$DOWNLOADS/main.go" || true
sleep 1
show_transfers
hover 640 700
shot 02-transfers-panel

for pair in bytes.bin:/home/gem/binary/bytes.bin pixel.png:/home/gem/binary/pixel.png main.go:/home/gem/samples/main.go; do
  name="${pair%%:*}"
  remote="${pair#*:}"
  local_sha="$(sha256sum "$DOWNLOADS/$name" 2>/dev/null | cut -d' ' -f1)"
  check "download $name is byte-exact" "$local_sha" "$(srv_sha "$remote")"
done
check "no partial files left in the download folder" "$(ls -A "$DOWNLOADS" | grep -c 'gem-part' || true)" "0"
assert_log_contains "downloads logged" "downloaded 65536 bytes from /home/gem/binary/bytes.bin" \
  "from /home/gem/binary/pixel.png" "from /home/gem/samples/main.go"

stop_gem
