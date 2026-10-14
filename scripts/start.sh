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
    "$TMUX_BIN" -S "$SOCKET" set-environment -gh TMUX_STATUS_LOCAL 'STATUS:unavailable' \; set-environment -gh TMUX_STATUS_REMOTE 'STATUS:unavailable' || true
    exit 1
  fi
fi
ARGS=(--serve "$SOCKET" --server-pid "$SERVER_PID" --tmux-bin "$TMUX_BIN"
  --interval "$(option @tmux-status-interval 5)"
  --battery-interval "$(option @tmux-status-battery-interval 60)"
  --probe-interval "$(option @tmux-status-network-interval 30)"
  --network-timeout-ms "$(option @tmux-status-network-timeout-ms 2000)")
if [ "$(option @tmux-status-adaptive on)" = off ]; then ARGS+=(--no-adaptive); fi
if [ "$(option @tmux-status-network on)" = on ]; then ARGS+=(--network); fi
if [ "$(option @tmux-status-network-direct off)" = on ]; then ARGS+=(--network-direct); fi
URLS="$(option @tmux-status-check-urls '')"
if [ -n "$URLS" ] && [ "$(option @tmux-status-network on)" = on ]; then
  IFS='|' read -r -a urls <<< "$URLS"
  for url in "${urls[@]}"; do ARGS+=(--check-url "$url"); done
fi
STYLE_FILE="$(option @tmux-status-appearance-file '')"
if [ -n "$STYLE_FILE" ]; then
  STYLE_FILE="${STYLE_FILE/#\~/$HOME}"
  ARGS+=(--appearance-file "$STYLE_FILE")
fi
ARGS+=(--appearance "$(option @tmux-status-appearance '{}')")
for key in cpu-label mem-label separator labels; do
  value="$(option "@tmux-status-$key" '')"
  if [ -n "$value" ]; then ARGS+=("--$key" "$value"); fi
 done
"$BIN" --check-config "${ARGS[@]}"
# Managed binary upgrades hand over to the new release without killing tmux or
# signalling a PID file. Verify the currently reported process's full command,
# both its cached binary prefix and this exact socket/server identity.
owner="$("$TMUX_BIN" -S "$SOCKET" display-message -p '#{TMUX_STATUS_SUPERVISOR_PID}')"
if [ -z "$owner" ] || ! kill -0 "$owner" 2>/dev/null; then
  owner="$("$TMUX_BIN" -S "$SOCKET" display-message -p '#{TMUX_STATUS_COLLECTOR_PID}')"
fi
# Legacy installed collectors still publish the old option during handoff.
if [ -z "$owner" ]; then owner="$(option @tmux-status-collector-pid '')"; fi
if [[ "$owner" =~ ^[1-9][0-9]*$ ]]; then
  running="$(ps -o command= -p "$owner" 2>/dev/null || true)"
  running="${running#"${running%%[![:space:]]*}"}"
  case "$running" in
    "$ROOT/bin/tmux-status-"*" --serve $SOCKET --server-pid $SERVER_PID "*)
      case "$running" in
        "$BIN "*) ;;
        *)
          # Re-read before signalling, in case an overlapping reload won.
          if [ "$(ps -o command= -p "$owner" 2>/dev/null | sed 's/^[[:space:]]*//')" = "$running" ]; then
            kill -TERM "$owner" 2>/dev/null || true
            # A stopped owner cannot handle TERM or release its lock until resumed.
            kill -CONT "$owner" 2>/dev/null || true
            for ((waited=0; waited<100; waited++)); do
              if ! kill -0 "$owner" 2>/dev/null; then break; fi
              sleep 0.05
            done
          fi
          ;;
      esac
      ;;
  esac
fi
if [ "${TMUX_STATUS_WORKER:-0}" = 1 ]; then
  exec "$BIN" "${ARGS[@]}"
fi
exec "$BIN" "${ARGS[@]}" --supervise "$ROOT/scripts/start.sh"
