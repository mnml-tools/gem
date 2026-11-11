# Presentation screenshots for the README: Gem at 2x (GDK_SCALE=2), a 1440×900 CSS-px window
# (2880×1800 pixels) on a large virtual output. THEME=light|dark. Run, servers up:
#   E2E_WIDTH=3000 E2E_HEIGHT=1900 THEME=light sh scripts/e2e/nested-session.sh \
#     scripts/e2e/presentation.sh target-e2e/presentation-light
# Writes docs/gem-screenshot-$THEME.png (PUBLISH=0 keeps it in the output folder only).
#
# The scene: three connected tabs (acme-api over SFTP, staging over SFTP, legacy-ftp over plain
# FTP), the synthetic project of presentation-seed.sh at /home/gem/acme-api (removed at the end),
# docker-compose.yml and InvoiceController.php open, a finished "Tail app log" snippet tab and
# the connection log. Clicks are in CSS pixels (doubled by `c`), measured on the screenshots.
export GDK_SCALE=2
WIN_W=2880
WIN_H=1800
. "$ROOT/scripts/e2e/lib.sh"
THEME="${THEME:-light}"
PROJECT=/home/gem/acme-api

cleanup() { docker exec "$SFTP_CT" rm -rf "$PROJECT" 2>/dev/null || true; }
trap 'on_exit; cleanup' EXIT

c() { click $(($1 * 2)) $(($2 * 2)); }
dc() { dclick $(($1 * 2)) $(($2 * 2)); }
away() { hover $((WIN_W + 60)) $((WIN_H + 60)); }

# connect_row <row from 0> <y of the first row>: a Sites page row → password prompt.
connect_row() {
  dc 300 $(($2 + $1 * 47))
  sleep 1.5
  type_text "$TEST_PASSWORD"
  key Return
  sleep 3
}

# goto_path <path>: Mod+L from the file list.
goto_path() {
  c 60 175
  key ctrl+l
  key ctrl+a
  type_text "$1"
  key Return
  sleep 1.5
}

docker exec -i -u gem "$SFTP_CT" sh -s "$PROJECT" "$(date +%s)" <"$ROOT/scripts/e2e/presentation-seed.sh"
check "project seeded" "$(docker exec "$SFTP_CT" ls -A "$PROJECT" | wc -l | tr -d ' ')" "21"

init_db
seed_site 01e2e000-0000-7000-8000-000000000001 "acme-api" sftp 2222 explicit "$PROJECT"
seed_site 01e2e000-0000-7000-8000-000000000002 "staging" sftp 2222 explicit /home/gem/samples
seed_site 01e2e000-0000-7000-8000-000000000003 "legacy-ftp" ftp 2121
trust_sftp
set_setting showHidden true   # .env and .env.example belong in the picture
sql "INSERT INTO snippets (id, site_id, name, command, confirm, sort_order, created_at, updated_at)
     VALUES ('01e2e000-0000-7000-8000-0000000000a1', NULL, 'Tail app log',
     'tail -n 40 {dir}/storage/logs/app.log', 0, 0, '2026-11-08T09:00:00Z', '2026-11-08T09:00:00Z')" >/dev/null

start_gem
if [ "$THEME" = dark ]; then
  c 1181 18                   # the palette button of the title bar (Sites page)
  sleep 0.8
  c 793 415                   # Appearance → Dark
  sleep 0.4
  c 842 548                   # Done
  sleep 0.8
fi

# ---- three connected tabs ----
connect_row 0 190             # acme-api, opens in the project
c 60 175; key ctrl+t; sleep 1
connect_row 1 222             # staging (the Sites tab sits below the tab strip)
c 60 175; key ctrl+t; sleep 1
connect_row 2 222             # legacy-ftp
c 60 175; key ctrl+1; sleep 1

# ---- files and the snippet in acme-api ----
c 92 607                      # docker-compose.yml (wide list)
key Return                    # pinned tab
sleep 1.5
# Widen the file list to 440 CSS px (its resize handle is the list's right edge), so the
# path bar shows the whole path next to the toolbar.
xdo mousemove --window "$WID" 761 800; sleep 0.2
xdo mousedown 1; sleep 0.2
xdo mousemove --window "$WID" 820 800; sleep 0.1
xdo mousemove --window "$WID" 881 800; sleep 0.2
xdo mouseup 1; sleep 0.5
goto_path "$PROJECT/app/Http/Controllers"
c 92 229                      # InvoiceController.php
key Return
sleep 1.5
goto_path "$PROJECT"
c 392 86                      # Snippets (the list is 440px wide now)
sleep 0.8
c 438 120                     # Tail app log
sleep 3
# The panel's controls at 2x (log_text reads the log through its Copy button).
PANEL_Y=1402; LOG_TAB_X=66; TRANSFERS_TAB_X=178; LOG_COPY_X=2692
assert_log_contains "the log shows the reads, the snippet and its exit" \
  "open $PROJECT/app/Http/Controllers/InvoiceController.php (read" \
  "exec tail -n 40 '$PROJECT'/storage/logs/app.log" "exit 0"
c 44 175                      # select the app folder (where the open controller lives)
c 703 85                      # back to InvoiceController.php (the focus leaves the list)
away
sleep 9                       # let the toasts (Plain FTP, Copied…) time out
shot "screenshot-$THEME"
check "screenshot size" "$(magick identify -format '%wx%h' "$OUT/screenshot-$THEME.png")" "2880x1800"

if [ "${PUBLISH:-1}" = 1 ]; then
  magick "$OUT/screenshot-$THEME.png" -strip -define png:compression-level=9 \
    -define png:compression-filter=5 -define png:compression-strategy=1 \
    "$ROOT/docs/gem-screenshot-$THEME.png"
  log "written: docs/gem-screenshot-$THEME.png"
fi
stop_gem
