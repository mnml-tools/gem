#!/usr/bin/env sh
# Runs every native end-to-end scenario, each in its own private KWin session, and prints a
# summary. Needs the release binary (pnpm tauri build --no-bundle) and the Docker test servers
# (pnpm servers:up). Output: target-e2e/<scenario>/ (screenshots, result.txt, logs) and
# target-e2e/summary.txt. Exit status 0 only when every assertion of every scenario passed.
#
# Usage: sh scripts/e2e/all.sh [scenario…]   (default: every scenario below, in order)
set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCENARIOS="${*:-sftp edit biglog tabs transfers dropped commands visual}"
SUMMARY="$ROOT/target-e2e/summary.txt"
mkdir -p "$ROOT/target-e2e"
{
  echo "Gem native e2e — $(date -Iseconds)"
  echo "binary: ${GEM_BIN:-$ROOT/src-tauri/target/release/gem}"
  echo "host: $(uname -srm); $(kwin_wayland --version 2>/dev/null | head -1)"
} >"$SUMMARY"

failed=0
for name in $SCENARIOS; do
  start=$(date +%s)
  if sh "$ROOT/scripts/e2e/nested-session.sh" "$ROOT/scripts/e2e/$name.sh" >/dev/null 2>&1; then
    status=PASS
  else
    status=FAIL
    failed=$((failed + 1))
  fi
  result="$ROOT/target-e2e/$name/result.txt"
  passes=$(grep -c '^PASS' "$result" 2>/dev/null)
  fails=$(grep -c '^FAIL' "$result" 2>/dev/null)
  line="$status $name (${passes:-0} passed, ${fails:-0} failed, $(( $(date +%s) - start ))s)"
  echo "$line" | tee -a "$SUMMARY"
  grep '^FAIL' "$result" 2>/dev/null | sed 's/^/    /' | tee -a "$SUMMARY"
done
echo "$([ "$failed" -eq 0 ] && echo ALL PASS || echo "$failed scenario(s) failed")" | tee -a "$SUMMARY"
[ "$failed" -eq 0 ]
