# Scenario: CA-06 / CA-10 (and CA-07's failed login) — three connection tabs at once:
# SFTP (OpenSSH), plain FTP (vsftpd, "Insecure") and explicit FTPS (ProFTPD, certificate trust),
# each browsing on its own; closing a tab with an unsaved file asks first.
. "$ROOT/scripts/e2e/lib.sh"

WRONG_PASSWORD=wrong-password-e2e

init_db
seed_site 01e2e000-0000-7000-8000-000000000001 "docker sftp" sftp 2222
seed_site 01e2e000-0000-7000-8000-000000000002 "vsftpd plain" ftp 2121
seed_site 01e2e000-0000-7000-8000-000000000003 "proftpd ftps" ftps 2321 explicit
trust_sftp
start_gem
shot 01-sites-insecure-badge          # CA-10: "Insecure" on the plain FTP site

# ---- tab 1: SFTP ----
connect_site 0
goto /home/gem/samples
shot 02-tab1-sftp

# ---- tab 2: plain FTP, a wrong password first (CA-07) ----
new_sites_tab
TAB_OFFSET=$TABS_H connect_site 1 "$WRONG_PASSWORD"
sleep 3                               # vsftpd answers a failed login after a delay
shot 03-ftp-login-failed
TEXT="$(log_text)"
printf '%s\n' "$TEXT" >"$OUT/ftp-failed-log.txt"
check_contains "CA-07 the server's refusal is in the log" "$TEXT" "530 Login incorrect."
check_contains "CA-07 the PASS command is masked" "$TEXT" "PASS ****"
case "$TEXT" in
  *"$WRONG_PASSWORD"*) fail "CA-07 the typed password never appears in the log" "found" ;;
  *) pass "CA-07 the typed password never appears in the log" ;;
esac
click 606 385                         # Retry (next to "Edit site") → the password prompt again
sleep 1.5
type_text "$TEST_PASSWORD"
key Return
sleep 3
hover 640 700
shot 04-tab2-ftp-insecure             # CA-10: "Insecure" in the tab and the title bar

goto /home/gem/encodings
preview utf8.txt
shot 04b-tab2-ftp-viewer              # viewing over plain FTP

# ---- tab 3: explicit FTPS with a self-signed certificate ----
click "$EDITOR_X" "$EDITOR_Y"         # Mod+T from inside the code editor (window-wide)
key ctrl+t
sleep 1
TAB_OFFSET=$TABS_H connect_site 2
shot 05-certificate-prompt
assert_sql "no certificate trusted before the user answers" \
  "SELECT count(*) FROM known_hosts WHERE port = 2321" "0"
click 789 493                         # Trust certificate
sleep 3
goto /home/gem/logs
hover 640 700
shot 06-three-tabs
assert_sql "CA-06 FTPS certificate pinned by its SHA-256 fingerprint" \
  "SELECT key_type || ' ' || fingerprint FROM known_hosts WHERE host = '127.0.0.1' AND port = 2321" \
  "x509 $(cert_fingerprint "$PROFTPD_CT" /etc/proftpd/tls/cert.pem)"

# ---- each tab keeps its own session, folder and log ----
focus_list; key ctrl+1; sleep 1; hover 640 700
shot 07-switch-tab1
T1="$(log_text)"
focus_list; key ctrl+2; sleep 1; hover 640 700
shot 08-switch-tab2
T2="$(log_text)"
focus_list; key ctrl+3; sleep 1; hover 640 700
shot 09-switch-tab3
T3="$(log_text)"
printf '%s\n' "$T1" >"$OUT/tab1-log.txt"; printf '%s\n' "$T2" >"$OUT/tab2-log.txt"; printf '%s\n' "$T3" >"$OUT/tab3-log.txt"
check_contains "CA-06 tab 1 is the SFTP session in samples" "$T1" "opendir /home/gem/samples"
check_contains "CA-06 tab 2 is the plain FTP session, connected after Retry" "$T2" '257 "/home/gem"'
check_contains "CA-06 tab 3 is the FTPS session in logs" "$T3" "MLSD /home/gem/logs"
case "$T1$T2" in *"/home/gem/logs"*) fail "CA-06 tabs 1 and 2 did not follow tab 3" "found /home/gem/logs" ;;
  *) pass "CA-06 tabs 1 and 2 did not follow tab 3" ;; esac
case "$T2$T3" in *"opendir /home/gem/samples"*) fail "CA-06 tabs 2 and 3 did not follow tab 1" "found" ;;
  *) pass "CA-06 tabs 2 and 3 did not follow tab 1" ;; esac

# ---- view a file over FTPS, make it dirty, Mod+W asks first ----
open_pinned small.log
click "$EDITOR_X" "$EDITOR_Y"
key ctrl+End
type_text "unsaved"
focus_list
key ctrl+w
sleep 1
shot 10-close-dirty-tab-asks
key Escape                            # keep the tab
sleep 1
hover 640 700
shot 11-tab-kept
kill -0 "$GEM_PID" 2>/dev/null && pass "Gem still running with three tabs" || fail "Gem still running with three tabs" "process gone"

stop_gem
assert_no_secret "Gem's own log files never show the password"
