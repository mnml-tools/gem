# Helpers for nested-session scenarios (sourced). See scripts/e2e/nested-session.sh.
# Every assertion appends one "PASS <what>" or "FAIL <what>: <why>" line to $OUT/result.txt.

GEM_DB="$GEM_DATA_DIR/gem.sqlite3"
SFTP_CT=gem-test-servers-sftp-1
FTP_CT=gem-test-servers-ftp-1
PROFTPD_CT=gem-test-servers-proftpd-1
# Test-only credentials of the Docker servers (dev/servers/README.md). Never real ones.
TEST_USER=gem
TEST_PASSWORD=gem-test-password
# Main window geometry (X11 pixels = CSS pixels at 96 dpi; the window has no decorations).
WIN_W="${WIN_W:-1280}"
WIN_H="${WIN_H:-800}"
GEM_PID=""

# A scenario that stops on an error (set -e) still records it and leaves nothing running.
on_exit() {
  code=$?
  if [ "$code" -ne 0 ]; then
    echo "FAIL scenario aborted (exit $code) after: $(tail -1 "$OUT/result.txt" 2>/dev/null)" >>"$OUT/result.txt"
    shot "zz-aborted" >/dev/null 2>&1 || true
  fi
  [ -n "$GEM_PID" ] && stop_gem
  return 0
}
trap on_exit EXIT

log() { echo "[e2e $(date +%T)] $*"; }

# ---- results ----------------------------------------------------------------------------

pass() { echo "PASS $1" >>"$OUT/result.txt"; log "PASS $1"; }
fail() { echo "FAIL $1: $2" >>"$OUT/result.txt"; log "FAIL $1: $2"; }

# check <description> <actual> <expected>
check() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1" "expected [$3], got [$2]"; fi
}

# check_contains <description> <haystack> <needle>
check_contains() {
  case "$2" in *"$3"*) pass "$1" ;; *) fail "$1" "[$3] not found in [$(printf %s "$2" | head -c 300)]" ;; esac
}

# ---- database ---------------------------------------------------------------------------

# sql <statement> [params…]: runs one statement against Gem's database and prints the first
# column of every row joined with "|".
sql() {
  python3 - "$GEM_DB" "$@" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1], timeout=5)
rows = db.execute(sys.argv[2], sys.argv[3:]).fetchall()
db.commit()
print("|".join(str(r[0]) for r in rows) if rows else "")
PY
}

assert_sql() {
  check "$1" "$(sql "$2")" "$3"
}

# init_db: creates the database with every schema migration before the first launch, so a
# scenario can seed sites and settings (Gem migrates a new file exactly like this).
init_db() {
  python3 - "$GEM_DB" "$ROOT"/src-tauri/migrations/*.sql <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
files = sorted(sys.argv[2:])
for path in files:
    db.executescript(open(path).read())
db.execute(f"PRAGMA user_version = {len(files)}")
db.commit()
PY
}

# seed_site <id> <name> <protocol> <port> [ftps_mode] [initial_path]: a password site on
# 127.0.0.1 for the test user, without a saved secret.
seed_site() {
  sql "INSERT INTO sites (id, name, protocol, host, port, user, auth, ftps_mode, initial_path,
       sort_order, has_secret, created_at, updated_at)
       VALUES (?, ?, ?, '127.0.0.1', ?, ?, 'password', ?, NULLIF(?, ''), ?, 0,
       '2026-11-08T09:00:00Z', '2026-11-08T09:00:00Z')" \
    "$1" "$2" "$3" "$4" "$TEST_USER" "${5:-explicit}" "${6:-}" "$(sql "SELECT count(*) FROM sites")" >/dev/null
}

# set_setting <camelCase key> <JSON value>
set_setting() {
  sql "INSERT INTO settings (key, value) VALUES (?, ?)
       ON CONFLICT (key) DO UPDATE SET value = excluded.value" "$1" "$2" >/dev/null
}

# ---- test servers -----------------------------------------------------------------------

# srv <command…>: runs a shell command as the test user in the SFTP container (all servers
# share the /home/gem volume).
srv() {
  docker exec -u gem "$SFTP_CT" sh -c "$*"
}
srv_cat() { docker exec "$SFTP_CT" cat "$1"; }
srv_sha() { docker exec "$SFTP_CT" sha256sum "$1" | cut -d' ' -f1; }
srv_stat() { docker exec "$SFTP_CT" stat -c '%a %U %G' "$1"; }

# scratch_dir <name>: a fresh scratch folder on the servers, /home/gem/it/e2e-<name>.
scratch_dir() {
  srv "rm -rf /home/gem/it/e2e-$1 && mkdir -p /home/gem/it/e2e-$1"
  echo "/home/gem/it/e2e-$1"
}

# wait_server <container> <seconds>: until the container is healthy again.
wait_server() {
  i=0
  until [ "$(docker inspect -f '{{.State.Health.Status}}' "$1" 2>/dev/null)" = healthy ]; do
    i=$((i + 1)); [ "$i" -gt "$(( $2 * 2 ))" ] && { log "$1 not healthy"; return 1; }
    sleep 0.5
  done
}

# ---- the app ----------------------------------------------------------------------------

# xdotool talks X11 (XTEST) to the nested XWayland, where Gem's window lives. Without this it
# would go through the Wayland RemoteDesktop portal and wait for a permission dialog.
xdo() {
  env -u WAYLAND_DISPLAY xdotool "$@"
}

# start_gem: launches Gem in the nested session (always via XWayland so xdotool can drive it),
# waits for its window and places it at the top-left corner at WIN_W×WIN_H.
# GDK_BACKEND=x11 is required: with WAYLAND_DISPLAY unset GTK would otherwise try the default
# socket "wayland-0", which is the user's real desktop.
start_gem() {
  [ -f "$GEM_DB" ] || init_db
  : >>"$OUT/gem.log"
  start_line="$(wc -l <"$OUT/gem.log")"
  env -u WAYLAND_DISPLAY GDK_BACKEND=x11 "$GEM_BIN" "$@" >>"$OUT/gem.log" 2>&1 &
  GEM_PID=$!
  i=0
  until tail -n +"$((start_line + 1))" "$OUT/gem.log" | grep -q "database ready"; do
    i=$((i + 1)); [ "$i" -gt 150 ] && { log "Gem did not start"; return 1; }
    kill -0 "$GEM_PID" 2>/dev/null || { log "Gem exited"; return 1; }
    sleep 0.2
  done
  wait_for_window "^Gem$" 20
  WID="$(xdo search --onlyvisible --name "^Gem$" | head -1)"
  xdo windowsize --sync "$WID" "$WIN_W" "$WIN_H" 2>/dev/null || xdo windowsize "$WID" "$WIN_W" "$WIN_H"
  xdo windowmove --sync "$WID" 0 0 2>/dev/null || xdo windowmove "$WID" 0 0
  sleep 1.5  # let the webview lay out and paint at the new size
  focus_gem
}

# Stops Gem and its WebKit helpers and waits for them, so nothing keeps writing to $OUT.
stop_gem() {
  [ -n "$GEM_PID" ] || return 0
  children="$(pgrep -P "$GEM_PID" 2>/dev/null || true)"
  kill "$GEM_PID" 2>/dev/null || true
  for child in $children; do kill "$child" 2>/dev/null || true; done
  i=0
  while kill -0 "$GEM_PID" 2>/dev/null && [ "$i" -lt 50 ]; do i=$((i + 1)); sleep 0.1; done
  GEM_PID=""
}

# wait_for_window <name regex> <seconds>
wait_for_window() {
  i=0
  while [ "$i" -lt "$(( $2 * 5 ))" ]; do
    if xdo search --onlyvisible --name "$1" >/dev/null 2>&1; then
      sleep 1.5  # let the webview paint
      return 0
    fi
    i=$((i + 1)); sleep 0.2
  done
  log "window '$1' did not appear"
  return 1
}

focus_gem() {
  xdo windowactivate --sync "$WID" 2>/dev/null || xdo windowfocus --sync "$WID" 2>/dev/null || true
}

# ---- layout (window-relative CSS pixels at WIN_W×WIN_H = 1280×800) ------------------------
# Found from the screenshots of the real build; see docs/e2e.md.

PANEL_Y=601          # bottom panel strip (Log / Transfers toggles and actions)
LOG_TAB_X=33
TRANSFERS_TAB_X=89
LOG_COPY_X=1186
FIRST_ROW_Y=175      # first row of the file list (below the path bar, filter and header)
FILTER_X=190         # filter field of the file list
FILTER_Y=120
SITE_ROW_X=300       # site rows of the Sites page: first one at SITE_ROW_Y, then every 46px
SITE_ROW_Y=190       # (add TABS_H when the Sites page is a tab)
SITE_ROW_H=46
TABS_H=32            # the connection tab strip, present once a tab is open
EDITOR_X=800         # somewhere inside the code editor
EDITOR_Y=300

# ---- input ------------------------------------------------------------------------------

# click <x> <y> [button]: clicks at window-relative coordinates (CSS pixels of the window).
click() {
  xdo mousemove --window "$WID" "$1" "$2"
  sleep 0.15
  xdo click "${3:-1}"
  sleep 0.35
}

dclick() {
  xdo mousemove --window "$WID" "$1" "$2"
  sleep 0.15
  xdo click --repeat 2 --delay 80 1
  sleep 0.35
}

rclick() { click "$1" "$2" 3; }

# hover <x> <y>: moves the pointer (window-relative), e.g. away from tooltips before a shot.
hover() { xdo mousemove --window "$WID" "$1" "$2"; sleep 0.2; }

# type_text <text>: types into the focused element.
type_text() {
  xdo type --delay 25 -- "$1"
  sleep 0.2
}

key() {
  xdo key --delay 60 "$@"
  sleep 0.25
}

# ---- screenshots and logs ---------------------------------------------------------------

# shot <name>: full screenshot of the virtual output (the window sits at 0,0).
shot() {
  spectacle -b -n -f -o "$OUT/$1.png" >/dev/null 2>&1
  if [ -n "${WID:-}" ] && command -v magick >/dev/null 2>&1; then
    magick "$OUT/$1.png" -crop "${WIN_W}x${WIN_H}+0+0" +repage "$OUT/$1.png" 2>/dev/null || true
  fi
  log "screenshot: $OUT/$1.png"
}

# pixel <x> <y> <shot name>: the "#rrggbb" colour of a pixel of a screenshot.
pixel() {
  magick "$OUT/$3.png" -format "%[hex:p{$1,$2}]" info: 2>/dev/null | cut -c1-6 | sed 's/^/#/'
}

# brightness <shot name> <x> <y>: 0–100, the lightness of a pixel of a screenshot.
brightness() {
  magick "$OUT/$1.png" -crop "1x1+$2+$3" +repage -colorspace Gray -format '%[fx:round(100*p{0,0}.intensity)]' info: 2>/dev/null
}

# assert_tone <description> <shot> <x> <y> light|dark
assert_tone() {
  b="$(brightness "$2" "$3" "$4")"
  case "$5" in
    light) [ "${b:-0}" -ge 80 ] && pass "$1" || fail "$1" "lightness $b at $3,$4 of $2 (expected >= 80)" ;;
    dark) [ "${b:-100}" -le 25 ] && pass "$1" || fail "$1" "lightness $b at $3,$4 of $2 (expected <= 25)" ;;
  esac
}

# pixels_where <shot> <x> <y> <w> <h> <awk condition on r g b>: how many pixels of a region of
# a screenshot match, e.g. "r > 150 && g < 110 && b < 110" (red text).
pixels_where() {
  magick "$OUT/$1.png" -crop "$4x$5+$2+$3" +repage -depth 8 -format %c histogram:info: 2>/dev/null |
    awk '
      { line = $0; n = $1 + 0; sub(/^[^(]*\(/, "", line); split(line, c, /[,)]/); r = c[1] + 0; g = c[2] + 0; b = c[3] + 0
        if (eval_cond(r, g, b)) total += n }
      END { print total + 0 }
      function eval_cond(r, g, b) { return '"$6"' }'
}
RED_TEXT='r > 150 && g < 110 && b < 110'
DANGER_TINT='r >= 240 && r - g >= 6 && r - b >= 6'

# app_log_file: Gem's own log file (tauri-plugin-log, under the app data dir of this run).
app_log_file() {
  find "$XDG_DATA_HOME" -name 'Gem*.log' 2>/dev/null | head -1
}

# assert_no_secret <description>: the test password appears in no log Gem wrote.
assert_no_secret() {
  files="$OUT/gem.log $(app_log_file)"
  # shellcheck disable=SC2086
  if grep -l -- "$TEST_PASSWORD" $files >/dev/null 2>&1; then
    fail "$1" "the password appears in $(grep -l -- "$TEST_PASSWORD" $files | tr '\n' ' ')"
  else
    pass "$1"
  fi
}

# assert_no_secret_in <description> <text>: the test password is not in <text>.
assert_no_secret_in() {
  case "$2" in *"$TEST_PASSWORD"*) fail "$1" "the password appears" ;; *) pass "$1" ;; esac
}

# clipboard: the text of the nested session's clipboard (what Gem copied).
clipboard() {
  env -u WAYLAND_DISPLAY GDK_BACKEND=x11 timeout 10 python3 "$ROOT/scripts/e2e/clipboard.py" 2>/dev/null
}

# The bottom panel: clicking the active view's toggle hides the panel (and moves its strip to
# the bottom), so the scenario keeps track of the view in a file (helpers run in subshells).
# A fresh run starts with the panel open on the Log view; it never changes by itself.
panel_view() { cat "$OUT/.panel-view" 2>/dev/null || echo log; }
show_log() {
  [ "$(panel_view)" = log ] && return 0
  click "$LOG_TAB_X" "$PANEL_Y"
  echo log >"$OUT/.panel-view"
}
show_transfers() {
  [ "$(panel_view)" = transfers ] && return 0
  click "$TRANSFERS_TAB_X" "$PANEL_Y"
  echo transfers >"$OUT/.panel-view"
}

# log_text: every line of the active connection tab's log, through the log's own Copy button
# (with no line selected it copies all of them). Leaves the panel open on the Log view.
log_text() {
  show_log
  click "$LOG_COPY_X" "$PANEL_Y"
  sleep 0.5
  clipboard
}

# assert_log_contains <description> <needle>…: the active tab's log has every needle.
assert_log_contains() {
  what="$1"; shift
  text="$(log_text)"
  printf '%s\n' "$text" >"$OUT/log-$(printf %s "$what" | tr -c 'A-Za-z0-9' '-' | cut -c1-40).txt"
  missing=""
  for needle in "$@"; do
    case "$text" in *"$needle"*) ;; *) missing="$missing [$needle]" ;; esac
  done
  if [ -z "$missing" ]; then pass "$what"; else fail "$what" "missing$missing"; fi
}

# wait_until <seconds> <command…>: polls a command until it succeeds.
wait_until() {
  limit=$(( $1 * 4 )); shift
  i=0
  until "$@" >/dev/null 2>&1; do
    i=$((i + 1)); [ "$i" -gt "$limit" ] && return 1
    sleep 0.25
  done
}

# ---- Gem flows ---------------------------------------------------------------------------

# sftp_fingerprint: the SFTP server's ed25519 host key fingerprint ("SHA256:…").
sftp_fingerprint() {
  docker exec "$SFTP_CT" ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub | cut -d' ' -f2
}

# cert_fingerprint <container> <pem path>: "SHA256:<base64, no padding>" of a TLS certificate.
cert_fingerprint() {
  docker exec "$1" cat "$2" | python3 -c '
import base64, hashlib, ssl, sys
der = ssl.PEM_cert_to_DER_cert(sys.stdin.read())
print("SHA256:" + base64.b64encode(hashlib.sha256(der).digest()).decode().rstrip("="))'
}

# trust_sftp: stores the SFTP server's host key as trusted (scenarios that test something else).
trust_sftp() {
  sql "INSERT OR REPLACE INTO known_hosts (host, port, key_type, fingerprint, added_at)
       VALUES ('127.0.0.1', 2222, 'ssh-ed25519', ?, '2026-11-08T09:00:00Z')" "$(sftp_fingerprint)" >/dev/null
}

# connect_site <row index from 0> [password]: double-clicks a site on the Sites page (window
# with no tab yet, or a Sites tab when TAB_OFFSET=$TABS_H) and answers the password prompt.
connect_site() {
  dclick "$SITE_ROW_X" $((SITE_ROW_Y + ${TAB_OFFSET:-0} + $1 * SITE_ROW_H))
  sleep 1.5
  type_text "${2:-$TEST_PASSWORD}"
  key Return
  sleep 2.5
}

# focus_list: gives the file list the keyboard focus (selects its first row).
focus_list() {
  click 60 "$FIRST_ROW_Y"
}

# goto <remote path>: edits the path bar (Mod+L from the file list) and goes there.
goto() {
  focus_list
  key ctrl+l
  key ctrl+a
  type_text "$1"
  key Return
  sleep 1.5
}

# filter_to <name>: filters the current folder down to <name>; it is then the first row.
filter_to() {
  click "$FILTER_X" "$FILTER_Y"
  key ctrl+a
  type_text "$1"
  sleep 0.6
}

# preview <name>: filters to a file and presses Space on it (preview tab).
preview() {
  filter_to "$1"
  click 60 "$FIRST_ROW_Y"
  key space
  sleep 1.5
}

# open_pinned <name>: filters to a file and presses Enter on it (pinned tab, editable).
open_pinned() {
  filter_to "$1"
  click 60 "$FIRST_ROW_Y"
  key Return
  sleep 1.5
}

# new_sites_tab: Mod+T from the file list (a Sites page in a new tab).
new_sites_tab() {
  focus_list
  key ctrl+t
  sleep 1
}
