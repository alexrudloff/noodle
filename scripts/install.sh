#!/bin/zsh
set -euo pipefail

function _noodle_install_is_repo_root() {
  local candidate="${1:-}"
  [[ -n "${candidate}" ]] || return 1
  [[ -f "${candidate}/Cargo.toml" ]] || return 1
  [[ -d "${candidate}/plugin" ]] || return 1
  [[ -d "${candidate}/config" ]] || return 1
  [[ -d "${candidate}/modules" ]] || return 1
}

function _noodle_install_require_command() {
  local command_name="${1:-}"
  command -v "${command_name}" >/dev/null 2>&1 || {
    print -u2 -- "noodle install error: required command not found: ${command_name}"
    exit 1
  }
}

typeset -g NOODLE_INSTALL_BOOTSTRAP_DIR=""
typeset -g repo_root=""

function _noodle_install_cleanup() {
  [[ -n "${NOODLE_INSTALL_BOOTSTRAP_DIR}" ]] || return 0
  rm -rf "${NOODLE_INSTALL_BOOTSTRAP_DIR}"
}

function _noodle_install_resolve_repo_root() {
  local script_path="${0:A}"
  local prompt_script_path="${${(%):-%N}:A}"
  local candidate=""
  local -a local_candidates

  local_candidates=(
    "${NOODLE_INSTALL_SOURCE_DIR:-}"
    "${PWD:A}"
    "${script_path:h:h}"
    "${prompt_script_path:h:h}"
  )
  for candidate in "${local_candidates[@]}"; do
    [[ -n "${candidate}" ]] || continue
    if _noodle_install_is_repo_root "${candidate}"; then
      repo_root="${candidate:A}"
      return 0
    fi
  done

  local repo_slug="${NOODLE_INSTALL_REPO_SLUG:-alexrudloff/noodle}"
  local ref="${NOODLE_INSTALL_REF:-main}"
  local archive_url="${NOODLE_INSTALL_ARCHIVE_URL:-https://codeload.github.com/${repo_slug}/tar.gz/${ref}}"
  local archive_path
  local -a extracted_dirs

  _noodle_install_require_command curl
  _noodle_install_require_command tar
  NOODLE_INSTALL_BOOTSTRAP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/noodle-install.XXXXXX")"
  archive_path="${NOODLE_INSTALL_BOOTSTRAP_DIR}/source.tar.gz"

  print -u2 -- "Fetching noodle source from ${archive_url}"
  curl -fsSL "${archive_url}" -o "${archive_path}"
  tar -xzf "${archive_path}" -C "${NOODLE_INSTALL_BOOTSTRAP_DIR}"
  extracted_dirs=("${NOODLE_INSTALL_BOOTSTRAP_DIR}"/*(/N))
  if (( ${#extracted_dirs} != 1 )); then
    print -u2 -- "noodle install error: expected a single extracted source directory from ${archive_url}"
    exit 1
  fi
  candidate="${extracted_dirs[1]}"
  if ! _noodle_install_is_repo_root "${candidate}"; then
    print -u2 -- "noodle install error: downloaded archive does not contain a valid noodle source tree"
    exit 1
  fi
  repo_root="${candidate:A}"
}

_noodle_install_resolve_repo_root
if [[ -n "${NOODLE_INSTALL_BOOTSTRAP_DIR}" ]]; then
  trap _noodle_install_cleanup EXIT INT TERM
fi
install_root="${NOODLE_INSTALL_ROOT:-${HOME}/.noodle}"
install_root="${install_root:A}"
launch_agent_label="com.noodle.daemon"
launch_agent_dir="${HOME}/Library/LaunchAgents"
launch_agent_plist="${launch_agent_dir}/${launch_agent_label}.plist"
socket_path="${install_root}/noodle.sock"
pid_path="${install_root}/noodle.pid"
stdout_log="${install_root}/daemon.stdout.log"
stderr_log="${install_root}/daemon.stderr.log"
zshrc_path="${ZDOTDIR:-${HOME}}/.zshrc"
daemon_command="export NOODLE_PIDFILE='${pid_path}'; exec '${install_root}/bin/noodle' daemon --socket '${socket_path}'"
launchctl_domain="gui/$(id -u)"
launchctl_bin="${NOODLE_LAUNCHCTL_BIN:-launchctl}"

cd "${repo_root}"
cargo build --release

mkdir -p "${install_root}/bin"
mkdir -p "${install_root}/scripts"
rm -rf "${install_root}/plugin" "${install_root}/config" "${install_root}/modules" "${install_root}/scripts"
rm -f "${socket_path}" "${pid_path}"
cp -R "${repo_root}/plugin" "${install_root}/plugin"
cp -R "${repo_root}/config" "${install_root}/config"
cp -R "${repo_root}/modules" "${install_root}/modules"
mkdir -p "${install_root}/scripts"
cp "${repo_root}/scripts/install.sh" "${install_root}/scripts/install.sh"
cp "${repo_root}/scripts/uninstall.sh" "${install_root}/scripts/uninstall.sh"
cp "${repo_root}/target/release/noodle" "${install_root}/bin/noodle"
codesign --force --sign - "${install_root}/bin/noodle" >/dev/null 2>&1

if [[ ! -f "${install_root}/config.json" ]]; then
  cp "${repo_root}/config/config.example.json" "${install_root}/config.json"
  config_created=1
else
  config_created=0
fi

python3 - "${install_root}/config.json" "${config_created}" <<'PY'
import json
import os
import sys
from pathlib import Path

default_soul = (
    "You are noodle, a concise, helpful, calm, and direct zsh assistant. "
    "You live inside the user's terminal and answer briefly in plain text. "
    "Be concise but complete enough to be useful, and include a short command "
    "or snippet when the user asks for one. Do not be theatrical or verbose."
)
default_chat_prompt = (
    "You are noodle, a local terminal agent. You help the user think, search, inspect files, "
    "read and edit code, run commands, and complete tasks using the tools available to you. "
    "You are workspace-aware when relevant, but not limited to software engineering or zsh. "
    "Be concise, practical, and action-oriented. When the user asks how to do something, "
    "answer with the command, snippet, or example directly unless live verification is "
    "actually required."
)
default_chat_tools = [
    "memory_query",
    "file_read",
    "path_search",
    "glob",
    "grep",
    "web_fetch",
    "web_search",
    "file_write",
    "file_edit",
    "shell_exec",
    "interactive_shell_start",
    "interactive_shell_read",
    "interactive_shell_write",
    "interactive_shell_key",
    "interactive_shell_close",
    "mcp_tools_list",
    "mcp_tool_call",
    "mcp_resources_list",
    "mcp_resource_read",
    "task_note_write",
    "agent_handoff_create",
]
default_permissions = {
    "read_only": "allow",
    "network_read": "allow",
    "local_write": "ask",
    "shell_exec": "ask",
    "interactive_shell": "ask",
    "external": "ask",
}
provider_defaults = {
    "openai_responses": {
        "base_url": "https://api.openai.com/v1",
        "model": "gpt-5.4",
        "reasoning_effort": "medium",
    },
    "openai_compatible": {
        "base_url": "",
        "model": "",
        "reasoning_effort": "",
    },
    "anthropic": {
        "base_url": "https://api.anthropic.com/v1",
        "model": "",
        "reasoning_effort": "",
    },
}


def resolve_prompt_streams():
    if sys.stdin.isatty() and sys.stdout.isatty():
        return sys.stdin, sys.stdout
    try:
        return open("/dev/tty", "r"), open("/dev/tty", "w")
    except OSError:
        return sys.stdin, sys.stdout


PROMPT_INPUT, PROMPT_OUTPUT = resolve_prompt_streams()


def prompting_available():
    return PROMPT_INPUT.isatty() and PROMPT_OUTPUT.isatty()


def disable_focus_reporting():
    if not prompting_available():
        return
    try:
        print("\x1b[?1004l", end="", file=PROMPT_OUTPUT, flush=True)
    except OSError:
        pass


def prompt_readline(prompt):
    print(prompt, end="", file=PROMPT_OUTPUT, flush=True)
    value = PROMPT_INPUT.readline()
    if not value:
        return ""
    return value.rstrip("\n")


def nested_get(root, dotted_key, default=None):
    current = root
    for part in dotted_key.split("."):
        if not isinstance(current, dict):
            return default
        current = current.get(part)
        if current is None:
            return default
    return current


def nested_set(root, dotted_key, value):
    parts = dotted_key.split(".")
    current = root
    for part in parts[:-1]:
        child = current.get(part)
        if not isinstance(child, dict):
            child = {}
            current[part] = child
        current = child
    current[parts[-1]] = value


def normalize_int(value, default):
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def sanitize_text(value):
    if value is None:
        return None
    cleaned = []
    index = 0
    length = len(value)
    while index < length:
        ch = value[index]
        if ch == "\x1b":
            index += 1
            if index >= length:
                break
            if value[index] == "[":
                index += 1
                while index < length:
                    next_ch = value[index]
                    index += 1
                    if "@" <= next_ch <= "~":
                        break
                continue
            if value[index] == "]":
                index += 1
                while index < length:
                    next_ch = value[index]
                    index += 1
                    if next_ch == "\x07":
                        break
                    if next_ch == "\x1b" and index < length and value[index] == "\\":
                        index += 1
                        break
                continue
            continue
        if ch.isprintable():
            cleaned.append(ch)
        index += 1
    return "".join(cleaned).strip()


def install_override(name):
    value = os.environ.get(name)
    if value is None:
        return None
    return sanitize_text(value)


def apply_install_overrides(data):
    overrides = {
        "NOODLE_INSTALL_PROVIDER": "provider",
        "NOODLE_INSTALL_BASE_URL": "base_url",
        "NOODLE_INSTALL_API_KEY": "api_key",
        "NOODLE_INSTALL_MODEL": "model",
        "NOODLE_INSTALL_REASONING_EFFORT": "reasoning_effort",
        "NOODLE_INSTALL_TIMEOUT_SECONDS": "timeout_seconds",
    }
    for env_name, key in overrides.items():
        value = install_override(env_name)
        if value is None:
            continue
        if key == "timeout_seconds":
            nested_set(data, key, normalize_int(value, 30))
        else:
            nested_set(data, key, value)


def prompt_yes_no(question, default):
    suffix = "Y/n" if default else "y/N"
    while True:
        answer = prompt_readline(f"{question} [{suffix}]: ").strip().lower()
        if not answer:
            return default
        if answer in {"y", "yes"}:
            return True
        if answer in {"n", "no"}:
            return False


def prompt_choice(question, options, default_key):
    print(question, file=PROMPT_OUTPUT)
    for index, (key, label) in enumerate(options, start=1):
        marker = " (default)" if key == default_key else ""
        print(f"  {index}. {label}{marker}", file=PROMPT_OUTPUT)
    while True:
        answer = sanitize_text(prompt_readline("Choose a provider: ")) or ""
        if not answer:
            return default_key
        if answer.isdigit():
            index = int(answer)
            if 1 <= index <= len(options):
                return options[index - 1][0]
        for key, _label in options:
            if answer == key:
                return key


def prompt_text(question, default=None, allow_empty=True):
    prompt = f"{question}: "
    if default not in (None, ""):
        prompt = f"{question} [{default}]: "
    value = prompt_readline(prompt)
    value = sanitize_text(value) or ""
    if value:
        return value
    if default not in (None, ""):
        return default
    if allow_empty:
        return ""
    return None


def prompt_required(question, default=None):
    while True:
        value = prompt_text(question, default=default, allow_empty=False)
        if value not in (None, ""):
            return value


def sanitize_llm_settings(data):
    for key in ("provider", "base_url", "api_key", "model", "reasoning_effort"):
        value = data.get(key)
        if isinstance(value, str):
            data[key] = sanitize_text(value)


def maybe_prompt_llm_settings(data, config_created):
    mode = os.environ.get("NOODLE_INSTALL_CONFIGURE_LLM", "auto").strip().lower()
    if mode in {"0", "false", "no", "off"}:
        return
    if not prompting_available():
        return
    disable_focus_reporting()

    current_provider = nested_get(data, "provider", "openai_responses")
    provider = prompt_choice(
        "Select the model provider for noodle:",
        [
            ("openai_responses", "OpenAI Responses API"),
            ("openai_compatible", "OpenAI-compatible chat API"),
            ("anthropic", "Anthropic Messages API"),
        ],
        current_provider if current_provider in provider_defaults else "openai_responses",
    )
    defaults = provider_defaults[provider]
    same_provider = provider == current_provider
    current_base_url = (
        nested_get(data, "base_url", defaults["base_url"]) if same_provider else defaults["base_url"]
    ) or defaults["base_url"]
    current_model = (
        nested_get(data, "model", defaults["model"]) if same_provider else defaults["model"]
    ) or defaults["model"]
    current_reasoning = (
        nested_get(data, "reasoning_effort", defaults["reasoning_effort"])
        if same_provider
        else defaults["reasoning_effort"]
    ) or defaults["reasoning_effort"]
    current_timeout = str(nested_get(data, "timeout_seconds", 30) or 30)

    data["provider"] = provider
    if provider == "openai_compatible":
        data["base_url"] = prompt_required(
            "Base URL",
            default=current_base_url or None,
        )
    else:
        data["base_url"] = prompt_text("Base URL", default=current_base_url)

    if provider == "openai_responses":
        data["model"] = prompt_text("Model", default=current_model or defaults["model"])
        data["reasoning_effort"] = prompt_text(
            "Reasoning effort",
            default=current_reasoning or defaults["reasoning_effort"],
        )
    elif provider == "openai_compatible":
        data["model"] = prompt_required(
            "Model (for example: gpt-4.1)",
            default=current_model or None,
        )
    else:
        data["model"] = prompt_required(
            "Model (for example: claude-sonnet-4-5)",
            default=current_model or None,
        )

    data["api_key"] = prompt_required("API key")

    timeout_value = prompt_text("Timeout seconds", default=current_timeout)
    data["timeout_seconds"] = normalize_int(timeout_value, 30)
    print(f"Configuring noodle for provider={provider} model={data['model']}.")

path = Path(sys.argv[1]).expanduser()
config_created = sys.argv[2] == "1"
data = json.loads(path.read_text())
sanitize_llm_settings(data)
soul = data.get("soul")

if not isinstance(soul, str) or not soul.strip():
    data["soul"] = default_soul
data["max_tokens"] = max(normalize_int(data.get("max_tokens"), 1024), 1024)
data["timeout_seconds"] = normalize_int(data.get("timeout_seconds"), 30)
if data["timeout_seconds"] < 30:
    data["timeout_seconds"] = 30

plugins = data.setdefault("plugins", {})
chat = plugins.setdefault("chat", {})
typos = plugins.setdefault("typos", {})
utils = plugins.setdefault("utils", {})
memory_plugin = plugins.setdefault("memory", {})
scripting = plugins.setdefault("scripting", {})
todo = plugins.setdefault("todo", {})
order = plugins.setdefault("order", ["utils", "memory", "scripting", "todo", "chat", "typos"])
if isinstance(order, list) and not order:
    order.extend(["utils", "memory", "scripting", "todo", "chat", "typos"])
elif isinstance(order, list):
    desired_prefix = ["utils", "memory", "scripting", "todo"]
    for index, plugin_id in enumerate(reversed(desired_prefix)):
        if plugin_id not in order:
            order.insert(0, plugin_id)
permissions = data.setdefault("permissions", {})
permission_classes = permissions.setdefault("classes", {})
for key, value in default_permissions.items():
    permission_classes.setdefault(key, value)
search = data.setdefault("search", {})
search.setdefault("provider", "duckduckgo_html")
brave = search.setdefault("brave", {})
brave.setdefault("api_key", "")
brave.setdefault("base_url", "https://api.search.brave.com/res/v1/web/search")
brave.setdefault("country", "us")
brave.setdefault("search_lang", "en")
memory = data.setdefault("memory", {})
modules = data.setdefault("modules", {})
todo_memory = memory.setdefault("todo", {})
todo_memory.setdefault("command_event_limit", 200)
module_paths = modules.setdefault("paths", ["~/.noodle/modules"])
if isinstance(module_paths, list):
    if "~/.noodle/modules" not in module_paths:
        module_paths.insert(0, "~/.noodle/modules")
else:
    modules["paths"] = ["~/.noodle/modules"]
chat.setdefault("include_tool_context", 0)
chat.setdefault("tool_calling", 1)
chat.setdefault("task_execution", 1)
if int(chat.get("max_tool_rounds", 0) or 0) < 24:
    chat["max_tool_rounds"] = 24
chat.setdefault("max_replans", 1)
chat_tools = chat.setdefault("uses_tools", default_chat_tools.copy())
if isinstance(chat_tools, list):
    for tool in default_chat_tools:
        if tool not in chat_tools:
            chat_tools.append(tool)
else:
    chat["uses_tools"] = default_chat_tools.copy()
tool_availability = chat.setdefault("tool_availability", {})
if isinstance(tool_availability, dict):
    for tool in default_chat_tools:
        tool_availability.setdefault(tool, True)
else:
    chat["tool_availability"] = {tool: True for tool in default_chat_tools}
chat.setdefault("exports_tools", ["chat.send"])
chat.setdefault("prompt", default_chat_prompt)
utils.setdefault("uses_tools", [])
utils.setdefault("tool_availability", {})
utils.setdefault("exports_tools", [])
memory_plugin.setdefault("uses_tools", [])
memory_plugin.setdefault("tool_availability", {})
memory_plugin.setdefault("exports_tools", [])
scripting.setdefault("uses_tools", [])
scripting.setdefault("tool_availability", {})
scripting.setdefault("exports_tools", [])
todo.setdefault("uses_tools", [])
todo.setdefault("tool_availability", {})
todo.setdefault("exports_tools", [])
typos.setdefault("uses_tools", [])
typos.setdefault("tool_availability", {})
typos.setdefault("exports_tools", [])

apply_install_overrides(data)
maybe_prompt_llm_settings(data, config_created)

path.write_text(json.dumps(data, indent=2) + "\n")
PY

python3 - "${zshrc_path}" "${install_root}" <<'PY'
from pathlib import Path
import sys

zshrc_path = Path(sys.argv[1]).expanduser()
install_root = Path(sys.argv[2]).expanduser()
start_marker = "# >>> noodle shell integration >>>"
end_marker = "# <<< noodle shell integration <<<"
integration_line = f'source "{install_root}/plugin/noodle.plugin.zsh"'

if zshrc_path.exists():
    lines = zshrc_path.read_text().splitlines()
else:
    lines = []

filtered = []
inside_block = False
for line in lines:
    stripped = line.strip()
    if stripped == start_marker:
        inside_block = True
        continue
    if stripped == end_marker:
        inside_block = False
        continue
    if inside_block:
        continue
    if "noodle.plugin.zsh" in stripped and stripped.startswith("source "):
        continue
    filtered.append(line)

if filtered and filtered[-1].strip():
    filtered.append("")
filtered.extend([start_marker, integration_line, end_marker])
zshrc_path.parent.mkdir(parents=True, exist_ok=True)
zshrc_path.write_text("\n".join(filtered) + "\n")
PY

mkdir -p "${launch_agent_dir}"
cat > "${launch_agent_plist}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${launch_agent_label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/zsh</string>
    <string>-lc</string>
    <string>${daemon_command}</string>
  </array>
  <key>KeepAlive</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
  <key>WorkingDirectory</key>
  <string>${install_root}</string>
  <key>StandardOutPath</key>
  <string>${stdout_log}</string>
  <key>StandardErrorPath</key>
  <string>${stderr_log}</string>
</dict>
</plist>
EOF

"${launchctl_bin}" bootout "${launchctl_domain}/${launch_agent_label}" >/dev/null 2>&1 || true
"${launchctl_bin}" bootout "${launchctl_domain}" "${launch_agent_plist}" >/dev/null 2>&1 || true
"${launchctl_bin}" remove "${launch_agent_label}" >/dev/null 2>&1 || true
"${launchctl_bin}" bootstrap "${launchctl_domain}" "${launch_agent_plist}"
"${launchctl_bin}" kickstart -k "${launchctl_domain}/${launch_agent_label}" >/dev/null 2>&1 || true

if [[ -t 1 ]]; then
  hello_style=$'\033[1;97m'
  reset_style=$'\033[0m'
else
  hello_style=''
  reset_style=''
fi

cat <<'EOF'
Installed noodle.

Optional:
export NOODLE_CONFIG="$HOME/.noodle/config.json"
export NOODLE_ENABLE_ERROR_FALLBACK=1
export NOODLE_AUTO_RUN=1

To skip installer prompts:
export NOODLE_INSTALL_CONFIGURE_LLM=0
EOF

printf '\nInstalled files at: %s\n' "${install_root}"
printf 'Launch agent: %s\n' "${launch_agent_plist}"
printf 'Shell rc: %s\n' "${zshrc_path}"
printf '\nSay hello to noodle with:\n'
printf '%s' "${hello_style}"
printf 'oo hello! my name is <yourname>\n\n'
printf '%s' "${reset_style}"
printf 'type /help for additional commands\n'

if [[ "${NOODLE_INSTALL_SKIP_SHELL_RELOAD:-0}" != "1" ]] && { : </dev/tty >/dev/tty; } 2>/dev/null; then
  print
  print -- "Reloading zsh so oo is available now..."
  exec zsh -i </dev/tty >/dev/tty 2>/dev/tty
fi

print
print -- "Run 'exec zsh' to load noodle in this shell, then say: oo hello! my name is <yourname>"
