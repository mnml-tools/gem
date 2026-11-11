# Scenario: CA-11 dropped connection over SFTP. With a dirty file tab, the SFTP server restarts;
# the keepalive (5 s here) notices, the tab shows "Disconnected · Reconnect" and keeps the edit;
# after reconnecting, Mod+S saves it.
. "$ROOT/scripts/e2e/lib.sh"

DIR="$(scratch_dir dropped)"
FILE="$DIR/config.yaml"
srv "cp /home/gem/samples/config.yaml $FILE"
LINE="dropped: kept"
EXPECTED_SHA="$( (srv_cat "$FILE"; printf '%s' "$LINE") | sha256sum | cut -d' ' -f1)"

init_db
seed_site 01e2e000-0000-7000-8000-000000000001 "docker sftp" sftp 2222
trust_sftp
set_setting keepaliveSecs 5
start_gem
connect_site 0
goto "$DIR"
open_pinned config.yaml
click "$EDITOR_X" "$EDITOR_Y"
key ctrl+End
type_text "$LINE"
sleep 0.5
shot 01-dirty

log "restarting the SFTP server"
docker restart -t 1 "$SFTP_CT" >/dev/null
wait_server "$SFTP_CT" 60
sleep 8                       # at least one keepalive period after the restart
hover 640 700
shot 02-disconnected
assert_log_contains "CA-11 the dropped connection is noticed and logged" "Connection lost" "Disconnected"
key ctrl+s                    # must not save while disconnected (the editor is read-only)
sleep 1
check "CA-11 nothing saved while disconnected" "$(srv_sha "$FILE")" \
  "$(docker exec "$SFTP_CT" sha256sum /home/gem/samples/config.yaml | cut -d' ' -f1)"

click 1230 85                 # Reconnect, in the notice strip
sleep 1.5
# The session keeps the password it connected with in memory, so Reconnect does not ask again.
sleep 3
hover 640 700
shot 04-reconnected
click "$EDITOR_X" "$EDITOR_Y"
key ctrl+s
sleep 2
shot 05-saved
check "CA-11 after reconnecting the kept edit is saved" "$(srv_sha "$FILE")" "$EXPECTED_SHA"
check "CA-11 no temp file left" "$(docker exec "$SFTP_CT" ls -A "$DIR" | grep -c 'gem-' || true)" "0"

stop_gem
assert_no_secret "Gem's own log files never show the password"
