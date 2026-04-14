#!/bin/zsh
set -euo pipefail

repo_root="${0:A:h:h}"
install_root="${NOODLE_INSTALL_ROOT:-${HOME}/.noodle}"
install_root="${install_root:A}"
launch_agent_label="com.noodle.daemon"
launch_agent_dir="${HOME}/Library/LaunchAgents"
launch_agent_plist="${launch_agent_dir}/${launch_agent_label}.plist"
socket_path="${install_root}/noodle.sock"
pid_path="${install_root}/noodle.pid"
stdout_log="${install_root}/daemon.stdout.log"
stderr_log="${install_root}/daemon.stderr.log"
daemon_command="export NOODLE_PIDFILE='${pid_path}'; exec '${install_root}/bin/noodle' daemon --socket '${socket_path}'"
launchctl_domain="gui/$(id -u)"

cd "${repo_root}"
cargo build --release

mkdir -p "${install_root}/bin"
rm -rf "${install_root}/plugin" "${install_root}/config" "${install_root}/modules"
rm -f "${socket_path}" "${pid_path}"
cp -R "${repo_root}/plugin" "${install_root}/plugin"
cp -R "${repo_root}/config" "${install_root}/config"
cp -R "${repo_root}/modules" "${install_root}/modules"
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
from getpass import getpass
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
        "model": "gpt-5",
        "reasoning_effort": "minimal",
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


def install_override(name):
    value = os.environ.get(name)
    if value is None:
        return None
    return value


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
            nested_set(data, key, normalize_int(value, 20))
        else:
            nested_set(data, key, value)


def prompt_yes_no(question, default):
    suffix = "Y/n" if default else "y/N"
    while True:
        answer = input(f"{question} [{suffix}]: ").strip().lower()
        if not answer:
            return default
        if answer in {"y", "yes"}:
            return True
        if answer in {"n", "no"}:
            return False


def prompt_choice(question, options, default_key):
    print(question)
    for index, (key, label) in enumerate(options, start=1):
        marker = " (default)" if key == default_key else ""
        print(f"  {index}. {label}{marker}")
    while True:
        answer = input("Choose a provider: ").strip()
        if not answer:
            return default_key
        if answer.isdigit():
            index = int(answer)
            if 1 <= index <= len(options):
                return options[index - 1][0]
        for key, _label in options:
            if answer == key:
                return key


def prompt_text(question, default=None, secret=False, allow_empty=True):
    prompt = f"{question}: "
    if default not in (None, ""):
        prompt = f"{question} [{default}]: "
    value = getpass(prompt) if secret else input(prompt)
    value = value.strip()
    if value:
        return value
    if default not in (None, ""):
        return default
    if allow_empty:
        return ""
    return None


def prompt_required(question, default=None, secret=False):
    while True:
        value = prompt_text(question, default=default, secret=secret, allow_empty=False)
        if value not in (None, ""):
            return value


def maybe_prompt_llm_settings(data, config_created):
    mode = os.environ.get("NOODLE_INSTALL_CONFIGURE_LLM", "auto").strip().lower()
    if mode in {"0", "false", "no", "off"}:
        return
    if not (sys.stdin.isatty() and sys.stdout.isatty()):
        return

    should_prompt = config_created
    if mode in {"1", "true", "yes", "on", "always"}:
        should_prompt = True
    elif not config_created:
        should_prompt = prompt_yes_no("Update model/provider settings now?", False)

    if not should_prompt:
        return

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
    current_timeout = str(nested_get(data, "timeout_seconds", 20) or 20)
    existing_api_key = nested_get(data, "api_key", "") or ""

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

    api_prompt = "API key"
    if same_provider and existing_api_key:
        api_prompt += " (leave blank to keep current)"
    api_key = prompt_text(api_prompt, secret=True, allow_empty=True)
    if api_key:
        data["api_key"] = api_key
    elif not (same_provider and existing_api_key):
        data["api_key"] = ""

    timeout_value = prompt_text("Timeout seconds", default=current_timeout)
    data["timeout_seconds"] = normalize_int(timeout_value, 20)
    print(f"Configured noodle for provider={provider} model={data['model']}.")

path = Path(sys.argv[1]).expanduser()
config_created = sys.argv[2] == "1"
data = json.loads(path.read_text())
soul = data.get("soul")

if not isinstance(soul, str) or not soul.strip():
    data["soul"] = default_soul
data["max_tokens"] = max(normalize_int(data.get("max_tokens"), 1024), 1024)

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

launchctl bootout "${launchctl_domain}" "${launch_agent_plist}" >/dev/null 2>&1 || true
launchctl bootstrap "${launchctl_domain}" "${launch_agent_plist}"
launchctl kickstart -k "${launchctl_domain}/${launch_agent_label}" >/dev/null 2>&1 || true

cat <<'EOF'
Installed noodle.

Add this to ~/.zshrc:

source "$HOME/.noodle/plugin/noodle.plugin.zsh"

Optional:
export NOODLE_CONFIG="$HOME/.noodle/config.json"
export NOODLE_ENABLE_ERROR_FALLBACK=1
export NOODLE_AUTO_RUN=1

To skip installer prompts:
export NOODLE_INSTALL_CONFIGURE_LLM=0
EOF

printf '\nInstalled files at: %s\n' "${install_root}"
printf 'Launch agent: %s\n' "${launch_agent_plist}"
