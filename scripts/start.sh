#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOCKET="$1"
SERVER_PID="$2"
TMUX_BIN="${TMUX_BIN:-tmux}"
option() { local value; value="$("$TMUX_BIN" -S "$SOCKET" show-option -gqv "$1")"; printf '%s' "${value:-$2}"; }
BIN="$(option @tmux-status-bin '')"
if [ -z "$BIN" ]; then
  if ! BIN="$(TMUX_STATUS_SERVER_PID="$SERVER_PID" "$ROOT/scripts/install.sh")"; then
    "$TMUX_BIN" -S "$SOCKET" set -gq @tmux-status-local 'STATUS:unavailable' \; set -gq @tmux-status-remote 'STATUS:unavailable' || true
    exit 1
  fi
fi
ARGS=(--serve "$SOCKET" --server-pid "$SERVER_PID" --tmux-bin "$TMUX_BIN"
  --interval "$(option @tmux-status-interval 5)"
  --battery-interval "$(option @tmux-status-battery-interval 60)"
  --probe-interval "$(option @tmux-status-network-interval 30)"
  --network-timeout-ms "$(option @tmux-status-network-timeout-ms 2000)")
if [ "$(option @tmux-status-network on)" = on ]; then ARGS+=(--network); fi
if [ "$(option @tmux-status-network-direct off)" = on ]; then ARGS+=(--network-direct); fi
URLS="$(option @tmux-status-check-urls '')"
if [ -n "$URLS" ] && [ "$(option @tmux-status-network on)" = on ]; then
  IFS='|' read -r -a urls <<< "$URLS"
  for url in "${urls[@]}"; do ARGS+=(--check-url "$url"); done
fi
exec "$BIN" "${ARGS[@]}"
