#!/usr/bin/env sh
# Runs a native end-to-end scenario inside a private, virtual KWin session (Linux only).
#
# The nested compositor has its own Wayland socket, XWayland display and D-Bus session, so the
# scenario drives Gem's real window (real Rust backend, real WebKitGTK webview, production CSP)
# with xdotool and takes screenshots with spectacle without touching the user's desktop.
# The private D-Bus session has no Secret Service: the keyring is unavailable inside it, so
# scenarios type passwords at the prompt and never tick "Remember".
#
# Usage: scripts/e2e/nested-session.sh <scenario.sh> [output-dir]
#   The scenario runs with DISPLAY set to the nested XWayland, GEM_BIN pointing at the binary,
#   OUT at the output directory (default target-e2e/<scenario>), GEM_DATA_DIR at a fresh data
#   directory and DOWNLOADS at a fresh download folder (the OS Downloads folder of the run).
#   By default the release binary is used (`pnpm tauri build --no-bundle`): it embeds the
#   frontend, starts deterministically and exercises the production CSP and capabilities.
#   The Docker test servers must be running (`pnpm servers:up`, dev/servers/README.md).
# Exit status: 0 when every assertion in $OUT/result.txt is PASS, 1 otherwise.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCENARIO="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
OUT="${2:-$ROOT/target-e2e/$(basename "$1" .sh)}"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac
WIDTH="${E2E_WIDTH:-1400}"
HEIGHT="${E2E_HEIGHT:-900}"

for tool in kwin_wayland dbus-run-session xdotool spectacle docker python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; exit 2; }
done
docker inspect -f '{{.State.Running}}' gem-test-servers-sftp-1 2>/dev/null | grep -q true \
  || { echo "test servers are not running (pnpm servers:up)" >&2; exit 2; }

# D-Bus activates services in the private session (xdg-desktop-portal-kde, ksecretd…) that
# outlive dbus-run-session. They inherit this run's XDG_DATA_HOME: stop exactly those.
reap() {
  for pid in $(pgrep -u "$(id -u)"); do
    if tr '\0' '\n' 2>/dev/null <"/proc/$pid/environ" | grep -qxF "XDG_DATA_HOME=$OUT/share"; then
      kill "$pid" 2>/dev/null || true
    fi
  done
}

# A previous run's helpers may still be exiting: stop them and retry the cleanup briefly.
reap
for _ in 1 2 3 4 5; do rm -rf "$OUT" 2>/dev/null && break; sleep 1; done
mkdir -p "$OUT/data" "$OUT/config" "$OUT/share" "$OUT/cache" "$OUT/downloads"
# Private config for the nested compositor only (the user's ~/.config is never touched):
# let xdotool drive XWayland windows without KWin's remote-input permission prompt.
cat >"$OUT/config/kwinrc" <<'CONF'
[Xwayland]
XwaylandEisNoPrompt=true
CONF
# The OS Downloads folder of this run (read by Gem through the XDG user dirs).
printf 'XDG_DOWNLOAD_DIR="%s"\n' "$OUT/downloads" >"$OUT/config/user-dirs.dirs"
export XDG_CONFIG_HOME="$OUT/config"
# Webview storage (localStorage: theme, panel layout) and the app log live under the app-data
# dir; WebKit caches under the cache dir: isolate both per run.
export XDG_DATA_HOME="$OUT/share" XDG_CACHE_HOME="$OUT/cache"
GEM_BIN="${GEM_BIN:-$ROOT/src-tauri/target/release/gem}"
[ -x "$GEM_BIN" ] || { echo "missing binary: $GEM_BIN (run: pnpm tauri build --no-bundle)" >&2; exit 2; }
export OUT GEM_BIN GEM_DATA_DIR="$OUT/data" DOWNLOADS="$OUT/downloads" ROOT
: >"$OUT/result.txt"

status=0
timeout "${E2E_TIMEOUT:-300}" dbus-run-session -- \
  kwin_wayland --virtual --xwayland --no-lockscreen --width "$WIDTH" --height "$HEIGHT" \
  --socket "gem-e2e-$$" --exit-with-session "sh -eu $SCENARIO" >"$OUT/kwin.log" 2>&1 || status=$?
reap
[ "$status" -eq 0 ] || echo "FAIL scenario exited with status $status (see $OUT/kwin.log)" >>"$OUT/result.txt"
grep -q . "$OUT/result.txt" || echo "FAIL scenario recorded no assertion" >>"$OUT/result.txt"
cat "$OUT/result.txt"
! grep -q '^FAIL' "$OUT/result.txt"
