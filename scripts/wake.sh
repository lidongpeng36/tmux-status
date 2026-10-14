#!/usr/bin/env bash
# Focus/attach events send a datagram, never rewrite tmux options.
set -euo pipefail
BIN="$(tmux -S "$1" display-message -p '#{TMUX_STATUS_BINARY}')"
if [ -n "$BIN" ] && [ -x "$BIN" ]; then
  exec "$BIN" --wake --serve "$1" --server-pid "$2"
fi
