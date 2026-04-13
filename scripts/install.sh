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
rm -rf "${install_root}/plugin" "${install_root}/config"
rm -f "${socket_path}" "${pid_path}"
cp -R "${repo_root}/plugin" "${install_root}/plugin"
cp -R "${repo_root}/config" "${install_root}/config"
cp "${repo_root}/target/release/noodle" "${install_root}/bin/noodle"
codesign --force --sign - "${install_root}/bin/noodle" >/dev/null 2>&1

if [[ ! -f "${install_root}/config.json" ]]; then
  cp "${repo_root}/config/config.example.json" "${install_root}/config.json"
fi

python3 - "${install_root}/config.json" <<'PY'
import json
import sys
from pathlib import Path

default_soul = (
    "You are noodle, a concise, helpful, calm, and direct zsh assistant. "
    "You live inside the user's terminal and answer briefly in plain text. "
    "Do not be theatrical or verbose."
)
default_chat_prompt = (
    "You are noodle, a local terminal agent. You help the user think, search, inspect files, "
    "read and edit code, run commands, and complete tasks using the tools available to you. "
    "You are workspace-aware when relevant, but not limited to software engineering or zsh. "
    "Be concise, practical, and action-oriented."
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

path = Path(sys.argv[1]).expanduser()
data = json.loads(path.read_text())
soul = data.get("soul")

if not isinstance(soul, str) or not soul.strip():
    data["soul"] = default_soul

plugins = data.setdefault("plugins", {})
chat = plugins.setdefault("chat", {})
typos = plugins.setdefault("typos", {})
utils = plugins.setdefault("utils", {})
memory_plugin = plugins.setdefault("memory", {})
todo = plugins.setdefault("todo", {})
order = plugins.setdefault("order", ["utils", "memory", "todo", "chat", "typos"])
if isinstance(order, list) and not order:
    order.extend(["utils", "memory", "todo", "chat", "typos"])
elif isinstance(order, list):
    desired_prefix = ["utils", "memory", "todo"]
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
todo_memory = memory.setdefault("todo", {})
todo_memory.setdefault("command_event_limit", 200)
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
todo.setdefault("uses_tools", [])
todo.setdefault("tool_availability", {})
todo.setdefault("exports_tools", [])
typos.setdefault("uses_tools", [])
typos.setdefault("tool_availability", {})
typos.setdefault("exports_tools", [])

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
EOF

printf '\nInstalled files at: %s\n' "${install_root}"
printf 'Launch agent: %s\n' "${launch_agent_plist}"
