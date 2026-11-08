#!/usr/bin/env bash

set -euo pipefail

if [[ "${1:-}" == --popup ]]; then
  client="$2"
  start="$3"
  shift 3
  state="$(mktemp -t tmux-expose.XXXXXXXX)"
  trap 'rm -f -- "${state}"' EXIT
  while true; do
    size="$(tmux display-message -p -c "${client}" '#{client_width} #{client_height}')"
    status=0
    tmux display-popup -e "TMUX_EXPOSE_RESUME=${state}" -e "TMUX_EXPOSE_OPEN_SIZE=${size}" \
      -e "TMUX_EXPOSE_START=${start}" -e "TMUX_EXPOSE_PLUGIN_PATH=${BASH_SOURCE[0]}" "$@" || status=$?
    if [[ "${status}" != 75 || ! -s "${state}" ]]; then
      exit "${status}"
    fi
  done
fi

key="$(tmux show-option -gqv @tmux-expose-key)"
key_table="$(tmux show-option -gqv @tmux-expose-key-table)"
width="$(tmux show-option -gqv @tmux-expose-width)"
height="$(tmux show-option -gqv @tmux-expose-height)"
anchor="$(tmux show-option -gqv @tmux-expose-anchor)"
style="$(tmux show-option -gqv @tmux-expose-style)"
border_style="$(tmux show-option -gqv @tmux-expose-border-style)"
selected_color="$(tmux show-option -gqv @tmux-expose-selected-color)"
attached_color="$(tmux show-option -gqv @tmux-expose-attached-color)"
inactive_color="$(tmux show-option -gqv @tmux-expose-inactive-color)"
attention_color="$(tmux show-option -gqv @tmux-expose-attention-color)"
waiting_color="$(tmux show-option -gqv @tmux-expose-waiting-color)"
working_color="$(tmux show-option -gqv @tmux-expose-working-color)"
agent_sort="$(tmux show-option -gqv @tmux-expose-agent-sort)"
vim_keys="$(tmux show-option -gqv @tmux-expose-vim-keys)"
command="$(tmux show-option -gqv @tmux-expose-command)"

if [[ -z "${key}" ]]; then
  key="M-e"
  key_table="${key_table:-root}"
else
  key_table="${key_table:-prefix}"
fi

width="${width:-100%}"
height="${height:-100%}"
anchor="${anchor:-center}"
appearance_file="${XDG_CONFIG_HOME:-${HOME}/.config}/tmux-expose/appearance.conf"
if [[ -r "${appearance_file}" ]]; then
  while IFS='=' read -r option value; do
    case "${option}" in
      key) key="${value}" ;;
      key-table) key_table="${value}" ;;
      width) width="${value}" ;;
      height) height="${value}" ;;
      anchor) anchor="${value}" ;;
      background) style="${style:+${style},}bg=${value}" ;;
      border) border_style="${border_style:+${border_style},}fg=${value}" ;;
    esac
  done < "${appearance_file}"
fi
command="${command:-tmux-expose}"
for option in topology-refresh pane-refresh active-pane-refresh max-captures; do
  value="$(tmux show-option -gqv "@tmux-expose-${option}")"
  if [[ -n "${value}" ]]; then
    command="${command} --${option} $(printf '%q' "${value}")"
  fi
done

# Shell-escape color values before splicing them into the -E command string.
# tmux runs that string through the shell, where an unquoted hex value such as
# "#ff8700" would otherwise be swallowed as a comment.
if [[ -n "${selected_color}" ]]; then
  command="${command} --selected-color $(printf '%q' "${selected_color}")"
fi

if [[ -n "${attached_color}" ]]; then
  command="${command} --attached-color $(printf '%q' "${attached_color}")"
fi

if [[ -n "${inactive_color}" ]]; then
  command="${command} --inactive-color $(printf '%q' "${inactive_color}")"
fi

if [[ -n "${attention_color}" ]]; then
  command="${command} --attention-color $(printf '%q' "${attention_color}")"
fi

if [[ -n "${waiting_color}" ]]; then
  command="${command} --waiting-color $(printf '%q' "${waiting_color}")"
fi

if [[ -n "${working_color}" ]]; then
  command="${command} --working-color $(printf '%q' "${working_color}")"
fi

case "$(printf '%s' "${vim_keys}" | tr '[:upper:]' '[:lower:]')" in
  on|true|1|yes) command="${command} --vim" ;;
esac

# Agent-status sorting defaults on; @tmux-expose-agent-sort off/false/0/no opts out.
case "$(printf '%s' "${agent_sort}" | tr '[:upper:]' '[:lower:]')" in
  off|false|0|no) command="${command} --no-agent-sort" ;;
esac

position_args=()
case "${anchor}" in
  center) ;;
  top) position_args=(-y '#{popup_pane_top}') ;;
  bottom) position_args=(-y '#{popup_pane_bottom}') ;;
  left) position_args=(-x '#{popup_pane_left}') ;;
  right) position_args=(-x '#{popup_pane_right}') ;;
  *)
    printf 'tmux.expose: invalid @tmux-expose-anchor: %s\n' "${anchor}" >&2
    exit 1
    ;;
esac

style_args=()
if [[ -n "${style}" ]]; then
  style_args+=(-s "${style}")
fi

if [[ -n "${border_style}" ]]; then
  style_args+=(-S "${border_style}")
fi

# display-popup does not expand formats in -e values. Expand the client and
# session at keypress time in run-shell, where each value can be shell-quoted.
# Double literal '#' so popup position formats reach display-popup intact and
# a format in a custom command stays part of that command.
script="${BASH_SOURCE[0]}"
if [[ "${script}" != /* ]]; then script="${PWD}/${script}"; fi
script="${script//#/##}"
launcher="bash '${script//\'/\'\\\'\'}' --popup #{q:client_name}"
popup_args() {
  local args='-c #{q:client_name} -t #{q:pane_id} -e TMUX_EXPOSE_CLIENT=#{q:client_name} -e TMUX_EXPOSE_SESSION=#{q:session_id}'
  local argument
  for argument in "$@" "${style_args[@]}" -e "TMUX_EXPOSE_TOGGLE_KEY=${key}" -E "${command}"; do
    argument="${argument//#/##}"
    args+=" '${argument//\'/\'\\\'\'}'"
  done
  printf '%s' "${args}"
}
popup_arguments="$(popup_args -w "${width}" -h "${height}" "${position_args[@]}")"
tmux bind-key -T "${key_table}" "${key}" run-shell -b "TMUX_EXPOSE_BINDING=overview ${launcher} 0 ${popup_arguments}"
# Shift-F12 is the terminal encoding used for Ctrl+, on tmux 3.4.
settings_arguments="$(popup_args -w 100% -h 100%)"
tmux bind-key -T root S-F12 run-shell -b "${launcher} 1 ${settings_arguments}"
tmux bind-key -T root M-w run-shell -b "${launcher} 2 ${popup_arguments}"
