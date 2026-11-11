# Scenario: CA-04 safe edit and CA-05 conflict over SFTP, on a scratch copy of config.yaml.
. "$ROOT/scripts/e2e/lib.sh"

DIR="$(scratch_dir edit)"
FILE="$DIR/config.yaml"
srv "cp /home/gem/samples/config.yaml $FILE && chmod 640 $FILE"
MODE_BEFORE="$(srv_stat "$FILE")"
LINE="# edited by gem e2e"
EXPECTED_SHA="$( (srv_cat "$FILE"; printf '%s' "$LINE") | sha256sum | cut -d' ' -f1)"

init_db
seed_site 01e2e000-0000-7000-8000-000000000001 "docker sftp" sftp 2222
trust_sftp
start_gem
connect_site 0
goto "$DIR"
open_pinned config.yaml
shot 01-opened

# ---- CA-04: type a line, Mod+S ----
click "$EDITOR_X" "$EDITOR_Y"
key ctrl+End                  # the empty line after the final newline
type_text "$LINE"
sleep 0.5
shot 02-dirty
key ctrl+s
sleep 2
shot 03-saved
check "CA-04 the server file has exactly the new content" "$(srv_sha "$FILE")" "$EXPECTED_SHA"
check "CA-04 permissions and owner unchanged" "$(srv_stat "$FILE")" "$MODE_BEFORE"
check "CA-04 no temp file left" "$(docker exec "$SFTP_CT" ls -A "$DIR" | grep -c 'gem-' || true)" "0"
assert_log_contains "CA-04 atomic save through a temp sibling and posix-rename" \
  "posix-rename $DIR/.config.yaml.gem-" "saved $FILE (" "atomic)"

# ---- CA-05: the file changes on the server, then Gem saves again ----
sleep 1                       # a later mtime, whatever the server's resolution
srv "printf 'external: change\n' >> $FILE"
EXTERNAL_SHA="$(srv_sha "$FILE")"
click "$EDITOR_X" "$EDITOR_Y"
key ctrl+End
type_text "  # local change"
key ctrl+s
sleep 1.5
shot 04-conflict-dialog
check "CA-05 nothing overwritten while the dialog asks" "$(srv_sha "$FILE")" "$EXTERNAL_SHA"
click 680 451                 # Reload (discard mine)
sleep 2
hover 640 700
shot 05-reloaded
check "CA-05 after Reload the server keeps the external content" "$(srv_sha "$FILE")" "$EXTERNAL_SHA"
check "CA-05 no temp file left" "$(docker exec "$SFTP_CT" ls -A "$DIR" | grep -c 'gem-' || true)" "0"
check "CA-05 the file was read again after Reload" \
  "$(log_text | grep -c "open $FILE (read")" "2"

# The reloaded editor is clean: saving again must not write anything new.
click "$EDITOR_X" "$EDITOR_Y"
key ctrl+s
sleep 1.5
check "CA-05 a clean editor saves nothing" "$(srv_sha "$FILE")" "$EXTERNAL_SHA"

stop_gem
