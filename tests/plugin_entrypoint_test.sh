#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

run_plugin() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap "rm -rf -- $(printf '%q' "${tmpdir}")" EXIT

  cat >"${tmpdir}/tmux" <<'FAKE_TMUX'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "show-option" ]]; then
  option_name="${@: -1}"
  case "${option_name}" in
    @tmux-expose-key) printf '%s' "${TMUX_EXPOSE_TEST_KEY:-}" ;;
    @tmux-expose-key-table) printf '%s' "${TMUX_EXPOSE_TEST_KEY_TABLE:-}" ;;
    @tmux-expose-width) printf '%s' "${TMUX_EXPOSE_TEST_WIDTH:-}" ;;
    @tmux-expose-height) printf '%s' "${TMUX_EXPOSE_TEST_HEIGHT:-}" ;;
    @tmux-expose-anchor) printf '%s' "${TMUX_EXPOSE_TEST_ANCHOR:-}" ;;
    @tmux-expose-style) printf '%s' "${TMUX_EXPOSE_TEST_STYLE:-}" ;;
    @tmux-expose-border-style) printf '%s' "${TMUX_EXPOSE_TEST_BORDER_STYLE:-}" ;;
    @tmux-expose-selected-color) printf '%s' "${TMUX_EXPOSE_TEST_SELECTED_COLOR:-}" ;;
    @tmux-expose-attached-color) printf '%s' "${TMUX_EXPOSE_TEST_ATTACHED_COLOR:-}" ;;
    @tmux-expose-inactive-color) printf '%s' "${TMUX_EXPOSE_TEST_INACTIVE_COLOR:-}" ;;
    @tmux-expose-vim-keys) printf '%s' "${TMUX_EXPOSE_TEST_VIM_KEYS:-}" ;;
    @tmux-expose-command) printf '%s' "${TMUX_EXPOSE_TEST_COMMAND:-}" ;;
    @tmux-expose-topology-refresh) printf '%s' "${TMUX_EXPOSE_TEST_TOPOLOGY_REFRESH:-}" ;;
    @tmux-expose-pane-refresh) printf '%s' "${TMUX_EXPOSE_TEST_PANE_REFRESH:-}" ;;
    @tmux-expose-active-pane-refresh) printf '%s' "${TMUX_EXPOSE_TEST_ACTIVE_PANE_REFRESH:-}" ;;
    @tmux-expose-max-captures) printf '%s' "${TMUX_EXPOSE_TEST_MAX_CAPTURES:-}" ;;
  esac
  exit 0
fi

if [[ "$1" == bind-key && "${5:-}" == run-shell && "${6:-}" == -b ]]; then
  if [[ "$4" == S-F12 || "$4" == M-w ]]; then
    mode=1
    if [[ "$4" == M-w ]]; then mode=2; fi
    [[ "$3" == root && "$7" == *"--popup #{q:client_name} ${mode} "* ]]
    exit
  fi
  # Exercise the shell boundary and check that the popup receives a client,
  # session and pane after tmux expands the keypress context.
  [[ "$7" == "TMUX_EXPOSE_BINDING=overview "* ]]
  printf '%q ' "${@:1:4}" >>"${TMUX_EXPOSE_TEST_OUTPUT}"
  expanded="${7//'#{q:client_name}'/"'/dev/pts/test client'"}"
  expanded="${expanded//'#{q:session_id}'/"'\$5'"}"
  expanded="${expanded//'#{q:pane_id}'/"'%7'"}"
  sh -c "${expanded//##/#}"
  exit
fi

if [[ "$1" == display-message ]]; then
  printf '80 24\n'
  exit
fi

if [[ "$1" == display-popup ]]; then
  [[ "$2" == -e && "$3" == TMUX_EXPOSE_RESUME=* && -f "${3#*=}" ]]
  [[ "$4" == -e && "$5" == 'TMUX_EXPOSE_OPEN_SIZE=80 24' ]]
  [[ "$6" == -e && "$7" == 'TMUX_EXPOSE_START=0' ]]
  [[ "$8" == -e && "$9" == TMUX_EXPOSE_PLUGIN_PATH=*/tmux.expose.tmux ]]
  shift 9
  [[ "$1" == -c && "$2" == '/dev/pts/test client' ]]
  [[ "$3" == -t && "$4" == '%7' ]]
  [[ "$5" == -e && "$6" == 'TMUX_EXPOSE_CLIENT=/dev/pts/test client' ]]
  [[ "$7" == -e && "$8" == 'TMUX_EXPOSE_SESSION=$5' ]]
  printf '%q ' display-popup "${@:9}" >>"${TMUX_EXPOSE_TEST_OUTPUT}"
  exit
fi

printf '%q ' "$@" >>"${TMUX_EXPOSE_TEST_OUTPUT}"
FAKE_TMUX
  chmod +x "${tmpdir}/tmux"
  if [[ "${TMUX_EXPOSE_TEST_SAVED:-}" ]]; then
    mkdir -p "${tmpdir}/config/tmux-expose"
    printf '%s\n' "${TMUX_EXPOSE_TEST_SAVED}" >"${tmpdir}/config/tmux-expose/appearance.conf"
  fi

  XDG_CONFIG_HOME="${tmpdir}/config" \
    TMUX_EXPOSE_TEST_OUTPUT="${tmpdir}/output" \
    TMUX_EXPOSE_TEST_KEY="${TMUX_EXPOSE_TEST_KEY:-}" \
    TMUX_EXPOSE_TEST_KEY_TABLE="${TMUX_EXPOSE_TEST_KEY_TABLE:-}" \
    TMUX_EXPOSE_TEST_WIDTH="${TMUX_EXPOSE_TEST_WIDTH:-}" \
    TMUX_EXPOSE_TEST_HEIGHT="${TMUX_EXPOSE_TEST_HEIGHT:-}" \
    TMUX_EXPOSE_TEST_ANCHOR="${TMUX_EXPOSE_TEST_ANCHOR:-}" \
    TMUX_EXPOSE_TEST_STYLE="${TMUX_EXPOSE_TEST_STYLE:-}" \
    TMUX_EXPOSE_TEST_BORDER_STYLE="${TMUX_EXPOSE_TEST_BORDER_STYLE:-}" \
    TMUX_EXPOSE_TEST_SELECTED_COLOR="${TMUX_EXPOSE_TEST_SELECTED_COLOR:-}" \
    TMUX_EXPOSE_TEST_ATTACHED_COLOR="${TMUX_EXPOSE_TEST_ATTACHED_COLOR:-}" \
    TMUX_EXPOSE_TEST_INACTIVE_COLOR="${TMUX_EXPOSE_TEST_INACTIVE_COLOR:-}" \
    TMUX_EXPOSE_TEST_VIM_KEYS="${TMUX_EXPOSE_TEST_VIM_KEYS:-}" \
    TMUX_EXPOSE_TEST_COMMAND="${TMUX_EXPOSE_TEST_COMMAND:-}" \
    PATH="${tmpdir}:${PATH}" \
    bash "${repo_root}/tmux.expose.tmux" || return
  tr -d '\n' <"${tmpdir}/output"
}

assert_equals() {
  local expected="$1"
  local actual="$2"

  if [[ "${actual}" != "${expected}" ]]; then
    printf 'Expected:\n%s\n\nActual:\n%s\n' "${expected}" "${actual}" >&2
    exit 1
  fi
}

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose\ --topology-refresh\ 1800\ --pane-refresh\ 1600\ --active-pane-refresh\ 350\ --max-captures\ 3 ' \
  "$(TMUX_EXPOSE_TEST_TOPOLOGY_REFRESH=1800 TMUX_EXPOSE_TEST_PANE_REFRESH=1600 TMUX_EXPOSE_TEST_ACTIVE_PANE_REFRESH=350 TMUX_EXPOSE_TEST_MAX_CAPTURES=3 run_plugin)"

assert_equals \
  'bind-key -T prefix E display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=E -E tmux-expose ' \
  "$(TMUX_EXPOSE_TEST_KEY=E run_plugin)"

assert_equals \
  'bind-key -T prefix R display-popup -w 60% -h 100% -y \#\{popup_pane_bottom\} -s bg=colour234\,bold\,bg=blue -S fg=yellow -e TMUX_EXPOSE_TOGGLE_KEY=R -E tmux-expose ' \
  "$(TMUX_EXPOSE_POPUP_HELPER="${CARGO_TARGET_DIR:-${repo_root}/target}/debug/tmux-expose" \
    TMUX_EXPOSE_TEST_KEY=E TMUX_EXPOSE_TEST_STYLE='bg=colour234,bold' \
    TMUX_EXPOSE_TEST_SAVED=$'key=R\nwidth=60%\nanchor=bottom\nbackground=red\nbackground=blue\nborder=yellow' \
    run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(TMUX_EXPOSE_POPUP_HELPER=/bin/false run_plugin)"

if result="$(TMUX_EXPOSE_POPUP_HELPER=/bin/false TMUX_EXPOSE_TEST_SAVED='key=M-r' run_plugin 2>&1)"; then
  printf 'Expected a failure when the binary cannot read saved settings\n' >&2
  exit 1
fi
[[ "${result}" == *"could not load saved popup settings"* ]]

assert_equals \
  'bind-key -T root C-e display-popup -w 80% -h 70% -e TMUX_EXPOSE_TOGGLE_KEY=C-e -E tmux-expose\ --columns\ 2 ' \
  "$(TMUX_EXPOSE_TEST_KEY=C-e TMUX_EXPOSE_TEST_KEY_TABLE=root TMUX_EXPOSE_TEST_WIDTH=80% TMUX_EXPOSE_TEST_HEIGHT=70% TMUX_EXPOSE_TEST_COMMAND='tmux-expose --columns 2' run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 50% -y \#\{popup_pane_bottom\} -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(TMUX_EXPOSE_TEST_ANCHOR=bottom TMUX_EXPOSE_TEST_HEIGHT=50% run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 50% -y \#\{popup_pane_top\} -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(TMUX_EXPOSE_TEST_ANCHOR=top TMUX_EXPOSE_TEST_HEIGHT=50% run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 50% -h 100% -x \#\{popup_pane_right\} -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(TMUX_EXPOSE_TEST_ANCHOR=right TMUX_EXPOSE_TEST_WIDTH=50% run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 50% -h 100% -x \#\{popup_pane_left\} -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(TMUX_EXPOSE_TEST_ANCHOR=left TMUX_EXPOSE_TEST_WIDTH=50% run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -s bg=colour234 -S fg=colour245 -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(TMUX_EXPOSE_TEST_STYLE='bg=colour234' TMUX_EXPOSE_TEST_BORDER_STYLE='fg=colour245' run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose\ --selected-color\ magenta ' \
  "$(TMUX_EXPOSE_TEST_SELECTED_COLOR=magenta run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose\ --selected-color\ magenta\ --attached-color\ blue\ --inactive-color\ colour245 ' \
  "$(TMUX_EXPOSE_TEST_SELECTED_COLOR=magenta TMUX_EXPOSE_TEST_ATTACHED_COLOR=blue TMUX_EXPOSE_TEST_INACTIVE_COLOR=colour245 run_plugin)"

# Hex values must be shell-escaped so the popup command isn't truncated at '#'.
assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose\ --selected-color\ \\#ff8700 ' \
  "$(TMUX_EXPOSE_TEST_SELECTED_COLOR='#ff8700' run_plugin)"

assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose\ --vim ' \
  "$(TMUX_EXPOSE_TEST_VIM_KEYS=on run_plugin)"

# An "off"/unset value must not append the flag.
assert_equals \
  'bind-key -T root M-e display-popup -w 100% -h 100% -e TMUX_EXPOSE_TOGGLE_KEY=M-e -E tmux-expose ' \
  "$(TMUX_EXPOSE_TEST_VIM_KEYS=off run_plugin)"
