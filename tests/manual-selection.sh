#!/usr/bin/env bash
# Run from Alacritty to validate real mouse selection without installing a build.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${TMUX_STATUS_BIN:-$ROOT/target/release/tmux-status}"
[ -x "$BIN" ] || { echo 'Build first: cargo build --release --locked' >&2; exit 1; }
BASE="$(mktemp -d "${TMPDIR:-/tmp}/tmux-status-selection.XXXXXX")"
INNER="$BASE/inner"
OUTER="$BASE/outer"
cleanup() {
    tmux -S "$OUTER" kill-server >/dev/null 2>&1 || true
    tmux -S "$INNER" kill-server >/dev/null 2>&1 || true
    rm -rf -- "$BASE"
}
trap cleanup EXIT
# Bash %q quoting matches the plugin's trusted path handling.
printf 'set -g default-terminal tmux-256color\nset -g status-right-length 120\nset -g status-right "#{tmux_status}"\nset -g @tmux-status-interval 1\nset -g @tmux-status-network off\nset -g @tmux-status-bin %q\nrun-shell %q\n' "$BIN" "$ROOT/tmux-status.tmux" > "$BASE/config"
tmux -S "$INNER" -f "$BASE/config" new-session -d -x 140 -y 24 /bin/sh -c 'printf "\033[HSTABLE SELECTION: select this sentence with Shift + mouse and hold for 15 seconds.\n\nThe status bar refreshes every second. Press Ctrl-C to finish this isolated test.\n"; exec sleep 3600'
tmux -S "$OUTER" -f /dev/null new-session -d -x 140 -y 24 env -u TMUX tmux -S "$INNER" attach-session -f ignore-size
env -u TMUX tmux -S "$OUTER" attach-session
