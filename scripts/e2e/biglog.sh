# Scenario: CA-03 huge log over SFTP: the 200 MiB logs/app.log opens partially and read-only,
# and "Load more" appends the next window.
. "$ROOT/scripts/e2e/lib.sh"

init_db
seed_site 01e2e000-0000-7000-8000-000000000001 "docker sftp" sftp 2222
trust_sftp
start_gem
connect_site 0
goto /home/gem/logs
preview app.log
shot 01-confirm-size          # "This file is 200 MB" with Open (above confirmAboveMib)
click 829 379                 # Open
sleep 3
hover 640 700
shot 02-partial               # "Showing 2.0 MB of 200 MB · Load more · Load all", read-only
assert_log_contains "CA-03 only the first 2 MiB window is read" \
  "file, 209715200 bytes" "open /home/gem/logs/app.log (read 2097152 bytes at 0)"
click 1162 546                # Load more
sleep 3
hover 640 700
shot 03-load-more             # "Showing 4.0 MB of 200 MB"
TEXT="$(log_text)"
check_contains "CA-03 Load more reads the next window" "$TEXT" \
  "open /home/gem/logs/app.log (read 2097152 bytes at 2097152)"
check "CA-03 nothing beyond the two windows was read" \
  "$(printf '%s\n' "$TEXT" | grep -c 'open /home/gem/logs/app.log (read')" "2"
check "CA-03 nothing downloaded" "$(ls -A "$DOWNLOADS" | wc -l)" "0"

stop_gem
