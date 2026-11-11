# Scenario: CA-01 / CA-02 / CA-07 over SFTP, from an empty Gem.
# Sites page → New site dialog → save (no keyring) → connect → password prompt → host-key
# prompt (SHA256 fingerprint) → trust → /home/gem → samples → Space on every sample → log.
# Also: with no Secret Service (the nested D-Bus session has none), "Remember" is unticked and
# disabled, the site is saved without a secret, and nothing is stored in plain text.
. "$ROOT/scripts/e2e/lib.sh"

init_db
set_setting showHidden true   # .env and .htaccess are hidden files
start_gem
shot 01-sites-empty

# ---- New site dialog (the UI path) ----
click 1221 63                 # New site
sleep 1
shot 02-site-dialog
type_text 127.0.0.1           # Host has the focus
click 764 288; key ctrl+a; type_text 2222
click 515 350; type_text "$TEST_USER"
click 403 444                 # untick "Remember in the system keyring" (no password typed)
click 515 559; type_text "docker sftp"
shot 03-site-dialog-filled
click 840 618                 # Save site
sleep 1.5
shot 04-sites-one
assert_sql "site saved from the dialog" \
  "SELECT protocol || ' ' || host || ':' || port || ' ' || user || ' ' || auth || ' secret=' || has_secret FROM sites" \
  "sftp 127.0.0.1:2222 gem password secret=0"

# ---- connect: password prompt, then the unknown host key ----
dclick "$SITE_ROW_X" "$SITE_ROW_Y"
sleep 1.5
shot 05-password-prompt
type_text "$TEST_PASSWORD"
key Return
sleep 2.5
shot 06-host-key-prompt
assert_sql "nothing trusted before the user answers" "SELECT count(*) FROM known_hosts" "0"
click 783 493                 # Trust and connect
sleep 2.5
shot 07-listing-home
FP="$(sftp_fingerprint)"
assert_sql "CA-01 host key stored with the server's SHA256 fingerprint" \
  "SELECT key_type || ' ' || fingerprint FROM known_hosts WHERE host = '127.0.0.1' AND port = 2222" \
  "ssh-ed25519 $FP"
assert_log_contains "CA-07 log: prompt fingerprint, auth and listing of /home/gem" \
  "Host key ssh-ed25519 $FP is not trusted yet" "is trusted" "Authenticated as gem" \
  "opendir /home/gem" "response"

# ---- CA-01 / CA-02: Space on every sample, highlighted in place ----
filter_to samples
dclick 60 "$FIRST_ROW_Y"
sleep 1.5
shot 08-samples
n=10
for file in config.yaml index.php main.go nginx.conf .env docker-compose.yml composer.json Dockerfile deploy.sh; do
  preview "$file"
  n=$((n + 1))
  shot "$n-viewer-$(printf %s "$file" | tr -c 'A-Za-z0-9' '_' | sed 's/^_*//')"
done
assert_log_contains "CA-01 every sample read from the server" \
  "open /home/gem/samples/config.yaml (read" "open /home/gem/samples/index.php (read" \
  "open /home/gem/samples/main.go (read" "open /home/gem/samples/nginx.conf (read" \
  "open /home/gem/samples/.env (read" "open /home/gem/samples/docker-compose.yml (read" \
  "open /home/gem/samples/composer.json (read" "open /home/gem/samples/Dockerfile (read" \
  "open /home/gem/samples/deploy.sh (read"
hover 640 700
shot 30-log-panel
LOG="$(log_text)"
printf '%s\n' "$LOG" >"$OUT/connection-log.txt"
assert_no_secret_in "CA-07 the connection log never shows the password" "$LOG"
# CA-01: viewing writes nothing locally (files stay in memory): no sample content on disk.
if grep -rlF -e 'Synthetic environment file for the Gem test servers' -e 'worker_connections' \
  "$OUT/data" "$OUT/share" "$OUT/cache" "$DOWNLOADS" >/dev/null 2>&1; then
  fail "CA-01 nothing written to the local disk" "$(grep -rlF -e 'worker_connections' "$OUT/data" "$OUT/share" "$OUT/cache" "$DOWNLOADS" | head -3)"
else
  pass "CA-01 nothing written to the local disk"
fi
check "CA-01 download folder untouched" "$(ls -A "$DOWNLOADS" | wc -l)" "0"

# ---- No Secret Service: "Remember" must fail clearly, never store in plain text ----
new_sites_tab
click 1221 95                 # New site (below the tab strip)
sleep 1
type_text 127.0.0.1
click 764 288; key ctrl+a; type_text 2222
click 515 350; type_text "$TEST_USER"
click 640 418; type_text "$TEST_PASSWORD"   # Remember is off and disabled: no keyring
click 515 559; type_text "keyring test"
shot 32-keyring-hint          # Remember unticked and disabled, with the hint
click 840 618                 # Save site
sleep 2
shot 31-keyring-unavailable
assert_sql "no keyring: the site is saved without a secret" \
  "SELECT has_secret FROM sites WHERE name = 'keyring test'" "0"
if grep -qaF -- "$TEST_PASSWORD" "$GEM_DB" "$GEM_DB-wal" 2>/dev/null; then
  fail "no keyring: the password is not stored in the database" "found in $GEM_DB"
else
  pass "no keyring: the password is not stored in the database"
fi
kill -0 "$GEM_PID" 2>/dev/null && pass "no keyring: Gem keeps running" || fail "no keyring: Gem keeps running" "process gone"

stop_gem
assert_no_secret "CA-07 Gem's own log files never show the password"
