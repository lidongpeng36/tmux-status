#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
for option in status-left status-right; do
  value="$(tmux show-option -gqv "$option")"
  local_placeholder='#{tmux_status}'
  remote_placeholder='#{tmux_status_remote}'
  local_format='#{E:@tmux-status-local}'
  remote_format='#{E:@tmux-status-remote}'
  value="${value//$local_placeholder/$local_format}"
  value="${value//$remote_placeholder/$remote_format}"
  tmux set-option -gq "$option" "$value"
done
if [ -z "$(tmux show-option -gqv @tmux-status-local)" ]; then
  tmux set -gq @tmux-status-local 'CPU:--  MEM:--' \; set -gq @tmux-status-remote 'CPU:--  MEM:--'
fi
tmux set -gq @tmux-status-version "$(cat "$ROOT/VERSION")"
# Shell escaping is confined to the trusted entry-point path; tmux expands the
# socket/server formats at job creation. The Rust owner deduplicates reloads.
printf -v start '%q' "$ROOT/scripts/start.sh"
command="$start \"#{socket_path}\" \"#{pid}\""
TAG=tmux-status-start-hook
# Keep other plugins' client-attached hooks. Reattaching also repairs a crashed
# collector; a healthy owner only receives the configuration and keeps its PID.
indices="$(tmux show-hooks -g client-attached | awk -v tag="$TAG" 'index($0,tag) {i=index($0,"[");j=index($0,"]"); print substr($0,i+1,j-i-1)}')"
for index in $indices; do tmux set-hook -gu "client-attached[$index]"; done
tmux set-hook -ga client-attached "run-shell -b ': $TAG; $command'"
tmux run-shell -b "$command"
