#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
for option in status-left status-right; do
  value="$(tmux show-option -gqv "$option")"
  local_placeholder='#{tmux_status}'
  remote_placeholder='#{tmux_status_remote}'
  # These values already contain final text/styles. Recursive E expansion
  # inherits status strftime processing and consumes a literal % before #.
  local_format='#{TMUX_STATUS_LOCAL}'
  remote_format='#{TMUX_STATUS_REMOTE}'
  value="${value//$local_placeholder/$local_format}"
  value="${value//$remote_placeholder/$remote_format}"
  # Migrate an already-interpolated pre-upgrade status without a full config reload.
  old_local='#{@tmux-status-local}' old_remote='#{@tmux-status-remote}'
  value="${value//$old_local/$local_format}"
  value="${value//$old_remote/$remote_format}"
  if [ "$(tmux show-option -gqv "$option")" != "$value" ]; then
    tmux set-option -gq "$option" "$value"
  fi
done
if [ -z "$(tmux display-message -p '#{TMUX_STATUS_LOCAL}')" ]; then
  tmux set-environment -gh TMUX_STATUS_LOCAL '-- #[fg=#61afef] #[default]  -- #[fg=#c678dd] #[default]' \; set-environment -gh TMUX_STATUS_REMOTE '-- #[fg=#61afef] #[default]  -- #[fg=#c678dd] #[default]'
fi
tmux set-environment -gh TMUX_STATUS_VERSION "$(cat "$ROOT/VERSION")"
# Shell escaping is confined to the trusted entry-point path; tmux expands the
# socket/server formats at job creation. The Rust owner deduplicates reloads.
printf -v start '%q' "$ROOT/scripts/start.sh"
command="exec $start \"#{socket_path}\" \"#{pid}\""
TAG=tmux-status-start-hook
# Keep other plugins' client-attached hooks. Reattaching also repairs a crashed
# collector; a healthy owner only receives the configuration and keeps its PID.
indices="$(tmux show-hooks -g client-attached | awk -v tag="$TAG" 'index($0,tag) {i=index($0,"[");j=index($0,"]"); print substr($0,i+1,j-i-1)}')"
for index in $indices; do tmux set-hook -gu "client-attached[$index]"; done
tmux set-hook -ga client-attached "run-shell -b ': $TAG; $command'"
tmux run-shell -b "$command"
