# noodle host plugin

if [[ -n "${NOODLE_PLUGIN_LOADED:-}" ]]; then
  return 0
fi
typeset -g NOODLE_PLUGIN_LOADED=1

typeset -g NOODLE_PLUGIN_DIR="${${(%):-%N}:A:h}"
typeset -g NOODLE_PROJECT_DIR="${NOODLE_PLUGIN_DIR:h}"
typeset -g NOODLE_HELPER="${NOODLE_HELPER:-$NOODLE_PROJECT_DIR/bin/noodle}"
typeset -g NOODLE_CONFIG="${NOODLE_CONFIG:-$HOME/.noodle/config.json}"
typeset -g NOODLE_SOCKET="${NOODLE_SOCKET:-$HOME/.noodle/noodle.sock}"
typeset -g NOODLE_PIDFILE="${NOODLE_PIDFILE:-$HOME/.noodle/noodle.pid}"
typeset -g NOODLE_LAST_COMMAND=""
typeset -g NOODLE_LAST_STATUS=0
typeset -g NOODLE_RETRY_DEPTH=0
typeset -g NOODLE_RETRY_HISTORY=""
typeset -g NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT=""
typeset -g NOODLE_RUNTIME_LOADED=0
typeset -g NOODLE_DEBUG="${NOODLE_DEBUG:-}"
typeset -g NOODLE_AUTO_RUN="${NOODLE_AUTO_RUN:-}"
typeset -g NOODLE_ENABLE_ERROR_FALLBACK="${NOODLE_ENABLE_ERROR_FALLBACK:-}"
typeset -g NOODLE_MAX_RETRY_DEPTH="${NOODLE_MAX_RETRY_DEPTH:-}"
typeset -g NOODLE_PLUGIN_ORDER="${NOODLE_PLUGIN_ORDER:-}"
typeset -g NOODLE_SELECTION_MODE="${NOODLE_SELECTION_MODE:-}"
typeset -g NOODLE_SLASH_COMMANDS="${NOODLE_SLASH_COMMANDS:-}"
typeset -g NOODLE_DEBUG_OVERRIDE="${NOODLE_DEBUG-__NOODLE_UNSET__}"
typeset -g NOODLE_AUTO_RUN_OVERRIDE="${NOODLE_AUTO_RUN-__NOODLE_UNSET__}"
typeset -g NOODLE_ENABLE_ERROR_FALLBACK_OVERRIDE="${NOODLE_ENABLE_ERROR_FALLBACK-__NOODLE_UNSET__}"
typeset -g NOODLE_MAX_RETRY_DEPTH_OVERRIDE="${NOODLE_MAX_RETRY_DEPTH-__NOODLE_UNSET__}"
typeset -g NOODLE_PLUGIN_ORDER_OVERRIDE="${NOODLE_PLUGIN_ORDER-__NOODLE_UNSET__}"
typeset -g NOODLE_SELECTION_MODE_OVERRIDE="${NOODLE_SELECTION_MODE-__NOODLE_UNSET__}"
typeset -g NOODLE_SLASH_COMMANDS_OVERRIDE="${NOODLE_SLASH_COMMANDS-__NOODLE_UNSET__}"
typeset -g NOODLE_RAW_SESSION_OUTPUT_ACTIVE=0
typeset -g NOODLE_PENDING_SLASH_COMMAND=""

function _noodle_reset_runtime_config() {
  emulate -L zsh
  NOODLE_RUNTIME_LOADED=0
  NOODLE_DEBUG=""
  NOODLE_AUTO_RUN=""
  NOODLE_ENABLE_ERROR_FALLBACK=""
  NOODLE_MAX_RETRY_DEPTH=""
  NOODLE_PLUGIN_ORDER=""
  NOODLE_SELECTION_MODE=""
  NOODLE_SLASH_COMMANDS=""
  [[ "$NOODLE_DEBUG_OVERRIDE" != "__NOODLE_UNSET__" ]] && NOODLE_DEBUG="$NOODLE_DEBUG_OVERRIDE"
  [[ "$NOODLE_AUTO_RUN_OVERRIDE" != "__NOODLE_UNSET__" ]] && NOODLE_AUTO_RUN="$NOODLE_AUTO_RUN_OVERRIDE"
  [[ "$NOODLE_ENABLE_ERROR_FALLBACK_OVERRIDE" != "__NOODLE_UNSET__" ]] && NOODLE_ENABLE_ERROR_FALLBACK="$NOODLE_ENABLE_ERROR_FALLBACK_OVERRIDE"
  [[ "$NOODLE_MAX_RETRY_DEPTH_OVERRIDE" != "__NOODLE_UNSET__" ]] && NOODLE_MAX_RETRY_DEPTH="$NOODLE_MAX_RETRY_DEPTH_OVERRIDE"
  [[ "$NOODLE_PLUGIN_ORDER_OVERRIDE" != "__NOODLE_UNSET__" ]] && NOODLE_PLUGIN_ORDER="$NOODLE_PLUGIN_ORDER_OVERRIDE"
  [[ "$NOODLE_SELECTION_MODE_OVERRIDE" != "__NOODLE_UNSET__" ]] && NOODLE_SELECTION_MODE="$NOODLE_SELECTION_MODE_OVERRIDE"
  [[ "$NOODLE_SLASH_COMMANDS_OVERRIDE" != "__NOODLE_UNSET__" ]] && NOODLE_SLASH_COMMANDS="$NOODLE_SLASH_COMMANDS_OVERRIDE"
}

function _noodle_load_runtime_config() {
  emulate -L zsh
  (( NOODLE_RUNTIME_LOADED )) && return 0
  local line key value
  local payload
  payload="$("$NOODLE_HELPER" runtime-config --config "$NOODLE_CONFIG" 2>/dev/null)" || payload=""
  while IFS='=' read -r key value; do
    case "$key" in
      debug) [[ -z "$NOODLE_DEBUG" ]] && NOODLE_DEBUG="$value" ;;
      auto_run) [[ -z "$NOODLE_AUTO_RUN" ]] && NOODLE_AUTO_RUN="$value" ;;
      enable_error_fallback) [[ -z "$NOODLE_ENABLE_ERROR_FALLBACK" ]] && NOODLE_ENABLE_ERROR_FALLBACK="$value" ;;
      max_retry_depth) [[ -z "$NOODLE_MAX_RETRY_DEPTH" ]] && NOODLE_MAX_RETRY_DEPTH="$value" ;;
      plugin_order) [[ -z "$NOODLE_PLUGIN_ORDER" ]] && NOODLE_PLUGIN_ORDER="$value" ;;
      selection_mode) [[ -z "$NOODLE_SELECTION_MODE" ]] && NOODLE_SELECTION_MODE="$value" ;;
      slash_commands) [[ -z "$NOODLE_SLASH_COMMANDS" ]] && NOODLE_SLASH_COMMANDS="$value" ;;
    esac
  done <<< "$payload"

  [[ -n "$NOODLE_DEBUG" ]] || NOODLE_DEBUG=0
  [[ -n "$NOODLE_AUTO_RUN" ]] || NOODLE_AUTO_RUN=1
  [[ -n "$NOODLE_ENABLE_ERROR_FALLBACK" ]] || NOODLE_ENABLE_ERROR_FALLBACK=0
  [[ -n "$NOODLE_MAX_RETRY_DEPTH" ]] || NOODLE_MAX_RETRY_DEPTH=2
  [[ -n "$NOODLE_PLUGIN_ORDER" ]] || NOODLE_PLUGIN_ORDER="utils memory todo chat typos"
  [[ -n "$NOODLE_SELECTION_MODE" ]] || NOODLE_SELECTION_MODE="select"
  [[ -n "$NOODLE_SLASH_COMMANDS" ]] || NOODLE_SLASH_COMMANDS="help status reload config memory todo"
  NOODLE_RUNTIME_LOADED=1
}

function _noodle_config_value() {
  "$NOODLE_HELPER" config-value --config "$NOODLE_CONFIG" --key "$1" --fallback "$2" 2>/dev/null
}

function _noodle_config_list() {
  "$NOODLE_HELPER" config-list --config "$NOODLE_CONFIG" --key "$1" --fallback "$2" 2>/dev/null
}

function _noodle_ensure_daemon() {
  emulate -L zsh
  [[ -x "$NOODLE_HELPER" ]]
}

function _noodle_debug_enabled() {
  _noodle_load_runtime_config
  [[ "$NOODLE_DEBUG" != "0" ]]
}

function _noodle_auto_run_enabled() {
  _noodle_load_runtime_config
  [[ "$NOODLE_AUTO_RUN" == "1" ]]
}

function _noodle_error_fallback_enabled() {
  _noodle_load_runtime_config
  [[ "$NOODLE_ENABLE_ERROR_FALLBACK" == "1" ]]
}

function _noodle_max_retry_depth() {
  _noodle_load_runtime_config
  print -r -- "$NOODLE_MAX_RETRY_DEPTH"
}

function noodle_log() {
  emulate -L zsh
  local level="$1"
  local module="$2"
  local message="$3"
  if [[ "$level" == "debug" ]]; then
    _noodle_debug_enabled || return 0
  fi
  print -P "%F{244}[noodle:${module}]%f ${message}" >&2
}

function noodle_log_debug() {
  noodle_log "debug" "$1" "$2"
}

function noodle_log_info() {
  noodle_log "info" "$1" "$2"
}

function noodle_log_error() {
  noodle_log "error" "$1" "$2"
}

function _noodle_avatar_render() {
  emulate -L zsh
  local frame="$1"
  local suffix="${2:-}"
  if [[ -n "$suffix" ]]; then
    printf '\r\033[38;2;107;63;160m%s\033[0m\t%s' "$frame" "$suffix" >&2
  else
    printf '\r\033[38;2;107;63;160m%s\033[0m' "$frame" >&2
  fi
}

function _noodle_avatar_clear() {
  emulate -L zsh
  printf '\r\033[2K' >&2
}

function _noodle_avatar_play() {
  emulate -L zsh
  local delay="$1"
  shift
  local frame
  for frame in "$@"; do
    _noodle_avatar_render "$frame"
    sleep "$delay"
  done
  _noodle_avatar_clear
}

function _noodle_avatar_wait() {
  emulate -L zsh
  local pid="$1"
  local frames=("oO" "Oo")
  local dots=("." ".." "...")
  local i=1
  local j=1
  while kill -0 "$pid" 2>/dev/null; do
    _noodle_avatar_render "${frames[i]}" "${dots[j]}"
    i=$(( (i % ${#frames}) + 1 ))
    j=$(( (j % ${#dots}) + 1 ))
    sleep 0.5
  done
  _noodle_avatar_clear
}

function _noodle_avatar_found() {
  _noodle_avatar_play 0.9 "oo"
  _noodle_avatar_play 0.6 "oO"
  _noodle_avatar_play 1.2 "OO"
}

function _noodle_avatar_error() {
  _noodle_avatar_play 0.6 "OO" "oo" "__"
}

function _noodle_avatar_confused() {
  _noodle_avatar_play 0.4 "OO" "oO" "Oo" "~~"
}

function _noodle_avatar_wink() {
  _noodle_avatar_play 0.22 "OO" "O-" "OO"
}

function _noodle_avatar_line() {
  emulate -L zsh
  _noodle_finish_raw_output_if_needed
  local frame="$1"
  local text="$2"
  local line
  while IFS= read -r line || [[ -n "$line" ]]; do
    _noodle_avatar_render "$frame" "$line"
    printf '\n' >&2
  done <<< "$text"
  if [[ -z "$text" ]]; then
    _noodle_avatar_render "$frame" ""
    printf '\n' >&2
  fi
}

function _noodle_transcript_line() {
  emulate -L zsh
  _noodle_finish_raw_output_if_needed
  local text="$1"
  printf '\033[38;2;107;63;160m  \033[0m\t%s\033[0m\n' "$text" >&2
}

function _noodle_finish_raw_output_if_needed() {
  emulate -L zsh
  if (( NOODLE_RAW_SESSION_OUTPUT_ACTIVE )); then
    printf '\033[0m\n' >&2
    NOODLE_RAW_SESSION_OUTPUT_ACTIVE=0
  fi
}

function _noodle_escape_arg() {
  local value="$1"
  value="${value//$'\n'/ }"
  print -r -- "$value"
}

function _noodle_decode_field() {
  local value="$1"
  [[ -z "$value" ]] && return 0
  printf '%s' "$value" | xxd -r -p 2>/dev/null
}

function _noodle_call_helper() {
  emulate -L zsh
  setopt local_options pipefail no_aliases no_bg_nice no_monitor
  _noodle_ensure_daemon || return 1
  local mode="$1"
  local input="$2"
  local exit_status="$3"
  local selected_command="${4:-}"
  local tmp
  tmp="$(mktemp "${TMPDIR:-/tmp}/noodle.XXXXXX")" || return 1

  "$NOODLE_HELPER" \
    --mode "$mode" \
    --input "$(_noodle_escape_arg "$input")" \
    --cwd "$PWD" \
    --shell "zsh" \
    --exit-status "$exit_status" \
    --recent-command "$(_noodle_escape_arg "$NOODLE_LAST_COMMAND")" \
    --selected-command "$(_noodle_escape_arg "$selected_command")" \
    --config "$NOODLE_CONFIG" \
    >"$tmp" 2>"$tmp.stderr" &
  local pid=$!
  _noodle_avatar_wait "$pid"
  wait "$pid"
  local helper_status=$?
  if _noodle_debug_enabled && [[ -s "$tmp.stderr" ]]; then
    cat "$tmp.stderr" >&2
  fi
  if (( helper_status != 0 )); then
    _noodle_avatar_error
    if [[ -s "$tmp.stderr" ]] && ! _noodle_debug_enabled; then
      cat "$tmp.stderr" >&2
    fi
    noodle_log_debug "host" "helper failed with status $helper_status"
    rm -f "$tmp" "$tmp.stderr"
    return "$helper_status"
  fi
  local result
  result="$(<"$tmp")"
  noodle_log_debug "host" "helper payload: $result"
  rm -f "$tmp" "$tmp.stderr"
  print -r -- "$result"
}

function _noodle_stream_helper() {
  emulate -L zsh
  setopt local_options pipefail no_aliases no_bg_nice no_monitor
  _noodle_ensure_daemon || return 1
  local mode="$1"
  local input="$2"
  local exit_status="$3"
  local selected_command="${4:-}"
  local stderr_file fifo helper_pid helper_status=0 line last_status=0

  stderr_file="$(mktemp "${TMPDIR:-/tmp}/noodle-stream.stderr.XXXXXX")" || return 1
  fifo="$(mktemp -u "${TMPDIR:-/tmp}/noodle-stream.out.XXXXXX")" || {
    rm -f "$stderr_file"
    return 1
  }
  mkfifo "$fifo" || {
    rm -f "$stderr_file"
    return 1
  }

  "$NOODLE_HELPER" \
    --stream \
    --mode "$mode" \
    --input "$(_noodle_escape_arg "$input")" \
    --cwd "$PWD" \
    --shell "zsh" \
    --exit-status "$exit_status" \
    --recent-command "$(_noodle_escape_arg "$NOODLE_LAST_COMMAND")" \
    --selected-command "$(_noodle_escape_arg "$selected_command")" \
    --config "$NOODLE_CONFIG" \
    >"$fifo" 2>"$stderr_file" &
  helper_pid=$!

  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" ]] && continue
    noodle_log_debug "host" "stream payload: $line"
    _noodle_handle_payload "$line"
    last_status=$?
  done <"$fifo"

  wait "$helper_pid"
  helper_status=$?

  if _noodle_debug_enabled && [[ -s "$stderr_file" ]]; then
    cat "$stderr_file" >&2
  fi
  if (( helper_status != 0 )); then
    _noodle_avatar_error
    if [[ -s "$stderr_file" ]] && ! _noodle_debug_enabled; then
      cat "$stderr_file" >&2
    fi
    noodle_log_debug "host" "stream helper failed with status $helper_status"
    rm -f "$fifo" "$stderr_file"
    return "$helper_status"
  fi

  rm -f "$fifo" "$stderr_file"
  return "$last_status"
}

function noodle_config_value() {
  _noodle_config_value "$1" "$2"
}

function noodle_config_list() {
  _noodle_config_list "$1" "$2"
}

function noodle_call_helper() {
  _noodle_call_helper "$1" "$2" "$3"
}

function noodle_execute() {
  _noodle_execute_command "$1" "$2"
}

function _noodle_execute_command() {
  emulate -L zsh
  local command="$1"
  local explanation="$2"
  [[ -n "$explanation" ]] && print -P "%F{244}${explanation}%f" >&2

  if _noodle_auto_run_enabled; then
    noodle_log_debug "host" "running inferred command: $command"
    local prev_depth="$NOODLE_RETRY_DEPTH"
    NOODLE_RETRY_DEPTH=$(( prev_depth + 1 ))
    eval "$command"
    local exit_code=$?
    NOODLE_RETRY_DEPTH="$prev_depth"
    return "$exit_code"
  fi

  print -P "%F{244}> ${command}%f"
  return 0
}

function _noodle_handle_payload() {
  emulate -L zsh
  local payload="$1"
  local action="" command="" question="" explanation="" message="" plugin="" text=""
  local permission_id="" permission_tool="" permission_class="" permission_summary=""
  local task_id="" task_status="" task_summary="" task_tool="" task_index="" task_total=""
  local -a choices items
  local line key value

  while IFS='=' read -r key value; do
    case "$key" in
      action) action="$value" ;;
      plugin) plugin="$(_noodle_decode_field "$value")" ;;
      command) command="$(_noodle_decode_field "$value")" ;;
      question) question="$(_noodle_decode_field "$value")" ;;
      explanation) explanation="$(_noodle_decode_field "$value")" ;;
      message) message="$(_noodle_decode_field "$value")" ;;
      text) text="$(_noodle_decode_field "$value")" ;;
      permission_id) permission_id="$(_noodle_decode_field "$value")" ;;
      tool) permission_tool="$(_noodle_decode_field "$value")" ;;
      permission_class) permission_class="$(_noodle_decode_field "$value")" ;;
      summary) permission_summary="$(_noodle_decode_field "$value")" ;;
      task_id) task_id="$(_noodle_decode_field "$value")" ;;
      status) task_status="$(_noodle_decode_field "$value")" ;;
      index) task_index="$value" ;;
      total) task_total="$value" ;;
      item) items+=("$(_noodle_decode_field "$value")") ;;
      choice) choices+=("$(_noodle_decode_field "$value")") ;;
    esac
  done <<< "$(printf '%s' "$payload" | "$NOODLE_HELPER" payload-fields 2>/dev/null)"

  task_summary="$permission_summary"
  task_tool="$permission_tool"

  case "$action" in
    run)
      [[ -n "$command" ]] || return 1
      _noodle_avatar_found
      _noodle_execute_command "$command" "$explanation"
      return $?
      ;;
    ask)
      _noodle_avatar_confused
      [[ -n "$question" ]] && _noodle_avatar_line "oo" "$question"
      return 1
      ;;
    message)
      _noodle_avatar_wink
      if [[ -n "$message" ]]; then
        _noodle_avatar_line "oo" "$message"
        if [[ "$message" == *$'\n'* ]]; then
          printf '\n' >&2
        fi
      fi
      return 0
      ;;
    reload_runtime)
      _noodle_reset_runtime_config
      _noodle_load_runtime_config
      _noodle_avatar_wink
      [[ -n "$message" ]] && _noodle_avatar_line "oo" "$message"
      return 0
      ;;
    session_started)
      [[ -n "$command" ]] && _noodle_avatar_line "oo" "\$ $command"
      return 0
      ;;
    session_input)
      if [[ -n "$text" ]]; then
        local input_text="$text"
        local input_line
        input_text="${input_text%$'\n'}"
        while IFS= read -r input_line || [[ -n "$input_line" ]]; do
          input_line="${input_line//$'\r'/}"
          _noodle_transcript_line "> $input_line"
        done <<< "$input_text"
      fi
      return 0
      ;;
    session_output)
      if [[ -n "$text" ]]; then
        if [[ "$text" == *$'\e['* ]]; then
          printf '%s\033[0m' "$text" >&2
          if [[ "$text" == *$'\n' ]]; then
            NOODLE_RAW_SESSION_OUTPUT_ACTIVE=0
          else
            NOODLE_RAW_SESSION_OUTPUT_ACTIVE=1
          fi
        else
          local output_line
          while IFS= read -r output_line || [[ -n "$output_line" ]]; do
            output_line="${output_line//$'\r'/}"
            _noodle_transcript_line "$output_line"
          done <<< "$text"
        fi
      fi
      return 0
      ;;
    session_closed)
      return 0
      ;;
    batch)
      local item_payload
      for item_payload in "${items[@]}"; do
        _noodle_handle_payload "$item_payload" || return $?
      done
      return 0
      ;;
    noop)
      return 0
      ;;
    permission_request)
      [[ -n "$permission_id" ]] || return 1
      _noodle_avatar_confused
      local prompt_text="$permission_summary"
      [[ -n "$prompt_text" ]] || prompt_text="Allow ${permission_tool} (${permission_class})?"
      _noodle_avatar_line "oo" "$prompt_text"
      local allow_choice
      read "allow_choice?Allow [Y/n]: " </dev/tty
      local decision="allow"
      case "$allow_choice:l" in
        n|no) decision="deny" ;;
      esac
      _noodle_stream_helper permission_response "$permission_id" 0 "$decision"
      return $?
      ;;
    task_started)
      [[ -z "$task_summary" ]] && task_summary="Starting task."
      _noodle_avatar_line "oO" "$task_summary"
      return 0
      ;;
    task_step)
      local task_text="$task_summary"
      [[ -z "$task_text" ]] && task_text="$task_tool"
      [[ -n "$task_text" ]] && _noodle_avatar_line "Oo" "$task_text"
      return 0
      ;;
    tool_step)
      local tool_text="$task_summary"
      [[ -z "$tool_text" ]] && tool_text="${permission_tool}"
      _noodle_avatar_line "Oo" "$tool_text"
      return 0
      ;;
    task_finished)
      return 0
      ;;
    select)
      (( ${#choices} > 0 )) || return 1
      _noodle_avatar_found
      if [[ "$NOODLE_SELECTION_MODE" == "auto" ]]; then
        if [[ "$plugin" == "typos" && -n "$NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT" ]]; then
          _noodle_call_helper typo_selected "$NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT" 0 "${choices[1]}" >/dev/null 2>&1 || true
        fi
        _noodle_execute_command "${choices[1]}" "$explanation"
        return $?
      fi
      local i=1 choice item
      _noodle_avatar_line "oo" "I found a few possibilities."
      for item in "${choices[@]}"; do
        print -u2 -- "$i. $item"
        i=$(( i + 1 ))
      done
      print -u2 -- "$i. abort"
      read "choice?Choose [1]: " </dev/tty
      if [[ -z "$choice" ]]; then
        if [[ "$plugin" == "typos" && -n "$NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT" ]]; then
          _noodle_call_helper typo_selected "$NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT" 0 "${choices[1]}" >/dev/null 2>&1 || true
        fi
        _noodle_execute_command "${choices[1]}" "$explanation"
        return $?
      fi
      if [[ "$choice" == "$i" ]]; then
        return 1
      fi
      if [[ "$choice" == <-> ]] && (( choice >= 1 && choice < i )); then
        if [[ "$plugin" == "typos" && -n "$NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT" ]]; then
          _noodle_call_helper typo_selected "$NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT" 0 "${choices[$choice]}" >/dev/null 2>&1 || true
        fi
        _noodle_execute_command "${choices[$choice]}" "$explanation"
        return $?
      fi
      return 1
      ;;
    *)
      noodle_log_debug "host" "unknown action payload: $payload"
      return 1
      ;;
  esac
}

function noodle_handle_payload() {
  _noodle_handle_payload "$1"
}

function _noodle_registered_slash_command_name() {
  emulate -L zsh
  _noodle_load_runtime_config
  local raw_input="$1"
  local trimmed="${raw_input#"${raw_input%%[![:space:]]*}"}"
  [[ "$trimmed" == /* ]] || return 1
  local name="${trimmed#/}"
  name="${name%%[[:space:]]*}"
  [[ -n "$name" ]] || return 1
  case " ${NOODLE_SLASH_COMMANDS} " in
    *" ${name} "*) print -r -- "$name"; return 0 ;;
  esac
  return 1
}

function _noodle_dispatch_explicit_input() {
  emulate -L zsh
  _noodle_load_runtime_config
  local raw_input="$1"
  local mode="command_not_found"
  if _noodle_registered_slash_command_name "$raw_input" >/dev/null; then
    mode="slash_command"
  fi
  noodle_log_debug "host" "forward explicit raw_input=[$raw_input] mode=[$mode] plugins=[$NOODLE_PLUGIN_ORDER]"
  _noodle_stream_helper "$mode" "$raw_input" 127
}

function _noodle_chat_oo() {
  emulate -L zsh
  local raw_input="oo"
  if (( $# > 0 )); then
    raw_input+=" $*"
  fi
  _noodle_dispatch_explicit_input "$raw_input"
}

function _noodle_chat_prefix() {
  emulate -L zsh
  local prefix
  prefix="$(_noodle_config_value "plugins.chat.prefix" ",")"
  [[ -n "$prefix" ]] || prefix=","
  local raw_input="$prefix"
  if (( $# > 0 )); then
    raw_input+=" $*"
  fi
  _noodle_dispatch_explicit_input "$raw_input"
}

unalias oo 2>/dev/null || true
alias oo='noglob _noodle_chat_oo'
unalias , 2>/dev/null || true
alias ,='noglob _noodle_chat_prefix'

function _noodle_accept_line_widget() {
  emulate -L zsh
  local raw_input="$BUFFER"
  if _noodle_registered_slash_command_name "$raw_input" >/dev/null; then
    print -s -- "$raw_input"
    NOODLE_PENDING_SLASH_COMMAND="$raw_input"
    BUFFER=""
    CURSOR=0
  fi
  zle accept-line
}

if [[ -o interactive ]] && whence zle >/dev/null 2>&1; then
  zle -N _noodle_accept_line_widget
  bindkey '^M' _noodle_accept_line_widget
  bindkey '^J' _noodle_accept_line_widget
fi

function command_not_found_handler() {
  emulate -L zsh
  _noodle_load_runtime_config
  local raw_input="$*"
  if (( NOODLE_RETRY_DEPTH >= NOODLE_MAX_RETRY_DEPTH )); then
    noodle_log_debug "host" "retry depth limit reached for: $raw_input"
    print -u2 -- "zsh: command not found: ${raw_input}"
    return 127
  fi
  case $'\n'"${NOODLE_RETRY_HISTORY}"$'\n' in
    *$'\n'"${raw_input}"$'\n'*)
      noodle_log_debug "host" "retry cycle detected for: $raw_input"
      print -u2 -- "zsh: command not found: ${raw_input}"
      return 127
      ;;
  esac
  local prev_history="$NOODLE_RETRY_HISTORY"
  NOODLE_RETRY_HISTORY="${prev_history}"$'\n'"${raw_input}"
  NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT="$raw_input"
  noodle_log_debug "host" "forward command_not_found raw_input=[$raw_input] plugins=[$NOODLE_PLUGIN_ORDER]"
  _noodle_stream_helper command_not_found "$raw_input" 127 || {
    NOODLE_RETRY_HISTORY="$prev_history"
    NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT=""
    print -u2 -- "zsh: command not found: ${raw_input}"
    return 127
  }
  local exit_code=$?
  NOODLE_RETRY_HISTORY="$prev_history"
  NOODLE_ACTIVE_COMMAND_NOT_FOUND_INPUT=""
  return "$exit_code"
}

function noodle-fix-last-error() {
  emulate -L zsh
  local exit_code="${1:-$NOODLE_LAST_STATUS}"
  local command="${2:-$NOODLE_LAST_COMMAND}"
  [[ -n "$command" ]] || {
    print -u2 -- "noodle: no recent command to inspect"
    return 1
  }
  local payload
  payload="$(_noodle_call_helper command_error "$command" "$exit_code")" || return 1
  _noodle_handle_payload "$payload"
}

function preexec() {
  NOODLE_LAST_COMMAND="$1"
}

function precmd() {
  NOODLE_LAST_STATUS="$?"
  if [[ -n "$NOODLE_PENDING_SLASH_COMMAND" ]]; then
    local raw_input="$NOODLE_PENDING_SLASH_COMMAND"
    NOODLE_PENDING_SLASH_COMMAND=""
    _noodle_dispatch_explicit_input "$raw_input"
  fi
}

function TRAPZERR() {
  emulate -L zsh
  local exit_code="$?"
  _noodle_error_fallback_enabled || return "$exit_code"
  [[ -n "$NOODLE_LAST_COMMAND" ]] || return "$exit_code"
  (( exit_code == 127 )) && return "$exit_code"
  case "$NOODLE_LAST_COMMAND" in
    *" "*)
      local payload
      payload="$(_noodle_call_helper command_error "$NOODLE_LAST_COMMAND" "$exit_code")" || return "$exit_code"
      _noodle_handle_payload "$payload" >/dev/null
      ;;
  esac
  return "$exit_code"
}
