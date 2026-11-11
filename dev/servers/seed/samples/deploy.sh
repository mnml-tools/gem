#!/usr/bin/env bash
# Synthetic deploy script: builds, uploads and restarts the demo service.
set -euo pipefail

readonly target="${1:-staging}"
readonly release="$(date +%Y%m%d%H%M%S)"

log() { printf '[%s] %s\n' "$(date +%T)" "$*"; }

if [[ "$target" != "staging" && "$target" != "production" ]]; then
  echo "usage: $0 [staging|production]" >&2
  exit 64
fi

log "building release ${release} for ${target}"
make build
for host in app-1 app-2; do
  log "deploying to ${host}"
  rsync -az ./dist/ "deploy@${host}.example.test:/srv/app/releases/${release}/"
done
log "done"
