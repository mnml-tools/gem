# Scenario: remote commands and snippets over SSH (ADR 0014).
# Manage snippets dialog → a global snippet (SQLite row with site_id NULL) and a site snippet that
# asks first; Snippets menu → output tab with the listing and Exit 0; Mod+K free command → Exit 3
# with stderr tinted; Mod+K `sleep 30` → Stop → Stopped, nothing left on the server; the
# confirmation shows the expanded command, Cancel runs nothing, Run creates the file; the
# connection log has the `exec` lines but never the output nor the password; an FTP tab has no
# Snippets menu and Mod+K runs nothing there.
. "$ROOT/scripts/e2e/lib.sh"

SFTP_ID=01e2e000-0000-7000-8000-000000000001
DIR="$(scratch_dir commands)"
docker exec "$SFTP_CT" pkill -f "sleep 30" 2>/dev/null || true

# Wide toolbar (no file open): the Snippets button and its menu items (global, then site).
SNIPPETS_WIDE_X=1231
SNIPPETS_NARROW_X=332         # once an output tab is open the list is 380px wide
TOOLBAR_Y=86
STATE_X=960                   # the state of an output tab ("Exit 0", "Stopped"…), in its bar
STATE_Y=106

# snippet_item <index from 0>: clicks a snippet in the Snippets menu of the narrow toolbar
# (the menu opens below the button, left-aligned; items every 28px).
snippet_item() {
  click "$SNIPPETS_NARROW_X" "$TOOLBAR_Y"
  sleep 0.6
  click $((SNIPPETS_NARROW_X + 54)) $((120 + $1 * 28))
}

# palette_run <text>: Mod+K, type, Enter on the first option ("Run <text>").
palette_run() {
  focus_list
  key ctrl+k
  sleep 0.6
  type_text "$1"
  sleep 0.3
}

# copy_output: the output of the active output tab, through its Copy output button.
copy_output() {
  click 1222 116
  sleep 0.5
  clipboard
}

init_db
seed_site "$SFTP_ID" "docker sftp" sftp 2222
seed_site 01e2e000-0000-7000-8000-000000000002 "vsftpd plain" ftp 2121
trust_sftp
start_gem
connect_site 0
shot 01-sftp-toolbar
check "the SFTP toolbar has the Snippets button" \
  "$([ "$(pixels_where 01-sftp-toolbar 1221 76 20 20 'r < 180')" -gt 0 ] && echo yes || echo no)" "yes"

# ---- Manage snippets: a global one, then a site one that asks first ----
click "$SNIPPETS_WIDE_X" "$TOOLBAR_Y"
sleep 0.6
shot 02-snippets-menu-empty
click 1124 176                # Manage snippets… (after "No snippets yet" and "Run command…")
sleep 1
shot 03-manage-empty
click 872 397                 # Global → Add snippet
sleep 0.8
type_text "List here"         # Name has the focus
click 640 396; type_text "ls -1 {dir}"
shot 04-new-global-snippet
click 876 548                 # Save snippet
sleep 1
assert_sql "global snippet saved with site_id NULL" \
  "SELECT name || ' | ' || command || ' | ' || ifnull(site_id, 'NULL') || ' | confirm=' || confirm FROM snippets" \
  "List here | ls -1 {dir} | NULL | confirm=0"
click 872 318                 # This site → Add snippet
sleep 0.8
type_text "Touch confirm"
click 640 396; type_text "touch {dir}/e2e-confirm.txt"
click 652 504                 # Ask before running
shot 05-new-site-snippet
click 876 548                 # Save snippet
sleep 1
hover 640 700
shot 06-manage-dialog
assert_sql "site snippet saved for the site, asking first" \
  "SELECT ifnull(site_id, 'NULL') || ' | confirm=' || confirm FROM snippets WHERE name = 'Touch confirm'" \
  "$SFTP_ID | confirm=1"
click 897 516                 # Done
sleep 0.5

# ---- the global snippet from the Snippets menu, in /home/gem/samples ----
goto /home/gem/samples
click "$SNIPPETS_WIDE_X" "$TOOLBAR_Y"
sleep 0.6
shot 07-snippets-menu
click 1098 120                # List here (global snippets come first)
sleep 2.5
hover 640 700
shot 08-output-exit0
OUT_LS="$(copy_output)"
printf '%s\n' "$OUT_LS" >"$OUT/output-list-here.txt"
check "List here lists the samples" "$(printf '%s\n' "$OUT_LS" | sort | tr '\n' ' ')" \
  "$(docker exec "$SFTP_CT" ls -1 /home/gem/samples | sort | tr '\n' ' ')"
check "List here ended with Exit 0 (no danger colour)" \
  "$(pixels_where 08-output-exit0 "$STATE_X" "$STATE_Y" 130 20 "$RED_TEXT")" "0"

# ---- Mod+K: a free command with stdout, stderr and exit 3 ----
# The output tokens are not in the command text, so the log can be checked for them.
palette_run "printf 'gem%s\n' -e2e-out; printf 'gem%s\n' -e2e-err >&2; exit 3"
shot 09-palette
key Return
sleep 2.5
hover 640 700
shot 10-output-exit3-stderr
OUT_3="$(copy_output)"
check "stdout and stderr both in the copied output, in order" "$(printf '%s' "$OUT_3" | tr '\n' ' ')" \
  "gem-e2e-out gem-e2e-err"
check "Exit 3 is shown in the danger colour" \
  "$([ "$(pixels_where 10-output-exit3-stderr "$STATE_X" "$STATE_Y" 130 20 "$RED_TEXT")" -gt 20 ] && echo yes || echo no)" "yes"
check "the stderr line is tinted, the stdout line is not" \
  "$([ "$(pixels_where 10-output-exit3-stderr 436 158 300 16 "$DANGER_TINT")" -gt 200 ] && echo yes || echo no) $(pixels_where 10-output-exit3-stderr 436 139 300 16 "$DANGER_TINT")" \
  "yes 0"

# ---- Mod+K: sleep 30, then Stop ----
palette_run "sleep 30"
key Return
sleep 2.5
hover 640 700
shot 11-running
check "sleep 30 runs on the server" "$(docker exec "$SFTP_CT" pgrep -f 'sleep 30' | wc -l | tr -d ' ')" "1"
click 1132 116                # Stop
sleep 3
hover 640 700
shot 12-stopped
check "Stopped is not shown as a failure" \
  "$(pixels_where 12-stopped "$STATE_X" "$STATE_Y" 130 20 "$RED_TEXT")" "0"
wait_until 5 sh -c "! docker exec $SFTP_CT pgrep -f 'sleep 30'" || true
check "Stop left no sleep 30 on the server" "$(docker exec "$SFTP_CT" pgrep -f 'sleep 30' | wc -l | tr -d ' ')" "0"

# ---- the site snippet asks first: Cancel runs nothing, Run creates the file ----
goto "$DIR"
snippet_item 1
sleep 1.2
shot 13-confirm-dialog         # shows touch '/home/gem/it/e2e-commands'/e2e-confirm.txt
click 725 474                 # Cancel
sleep 1.5
check "Cancel: the file was not created" "$(docker exec "$SFTP_CT" ls -A "$DIR" | wc -l | tr -d ' ')" "0"
snippet_item 1
sleep 1.2
click 782 474                 # Run
sleep 2.5
hover 640 700
shot 14-confirmed-ran
check "Run: the file was created on the server" "$(docker exec "$SFTP_CT" ls -A "$DIR")" "e2e-confirm.txt"
srv "rm -f $DIR/e2e-confirm.txt"

# ---- the connection log: exec lines, never the output, never the password ----
LOG="$(log_text)"
printf '%s\n' "$LOG" >"$OUT/connection-log.txt"
check_contains "log: the snippet's exec line with {dir} expanded" "$LOG" "exec ls -1 '/home/gem/samples'"
check_contains "log: the free command's exec line and exit 3" "$LOG" "exit 3"
check_contains "log: the confirmed snippet's exec line" "$LOG" "exec touch '$DIR'/e2e-confirm.txt"
check_contains "log: the stop" "$LOG" "Stopping the command (TERM)"
check "log: exactly one touch ran (Cancel ran nothing)" "$(printf '%s\n' "$LOG" | grep -c 'exec touch')" "1"
for token in gem-e2e-out gem-e2e-err style.css; do
  case "$LOG" in *"$token"*) fail "log: no command output ($token)" "found" ;; *) pass "log: no command output ($token)" ;; esac
done
assert_no_secret_in "log: never the password" "$LOG"

# ---- FTP: no Snippets menu, and Mod+K runs nothing ----
new_sites_tab
TAB_OFFSET=$TABS_H connect_site 1
hover 640 700
shot 15-ftp-toolbar
check "the FTP toolbar has no Snippets button" "$(pixels_where 15-ftp-toolbar 1086 76 20 20 'r < 180')" "0"
EXEC_BEFORE="$(log_text | grep -c 'exec ' || true)"
focus_list
key ctrl+k
sleep 0.8
shot 16-ftp-palette-unavailable  # "Commands need an SSH (SFTP) connection."
type_text "echo gem-ftp-probe"
key Return
sleep 1.5
key Escape
sleep 0.5
FTP_LOG="$(log_text)"
check "FTP: Mod+K ran nothing" "$(printf '%s\n' "$FTP_LOG" | grep -c 'exec ' || true)" "$EXEC_BEFORE"
check "FTP: no command was sent to the server" "$(printf '%s\n' "$FTP_LOG" | grep -c 'gem-ftp-probe' || true)" "0"

stop_gem
assert_no_secret "Gem's own log files never show the password"
