#!/usr/bin/env python3
import json
import os
import shlex
import sys

UTILS_PLUGIN = "utils"


def respond(ok, payload=None, error=None, stream=False):
    if stream:
        envelope = {
            "type": "final" if ok else "error",
            "ok": ok,
            "payload": payload,
            "error": error,
        }
        sys.stdout.write(json.dumps(envelope) + "\n")
    else:
        sys.stdout.write(json.dumps({"ok": ok, "payload": payload, "error": error}))
    sys.stdout.flush()


def expand_home(path):
    return os.path.expandvars(os.path.expanduser(path))


def parse_config_value(raw):
    try:
        return json.loads(raw)
    except Exception:
        return raw


def render_value_inline(value):
    if isinstance(value, str):
        return value
    return json.dumps(value, separators=(",", ":"))


def split_config_key(key):
    parts = [part.strip() for part in key.split(".") if part.strip()]
    if not parts:
        raise ValueError("Config key cannot be empty")
    return parts


def lookup(value, key):
    current = value
    for segment in split_config_key(key):
        if not isinstance(current, dict) or segment not in current:
            return None
        current = current[segment]
    return current


def set_path_value(root, key, value):
    segments = split_config_key(key)
    current = root
    for segment in segments[:-1]:
        child = current.get(segment)
        if child is None:
            child = {}
            current[segment] = child
        if not isinstance(child, dict):
            raise ValueError(f"Cannot write into non-object path: {key}")
        current = child
    current[segments[-1]] = value


def remove_path_value(root, key):
    segments = split_config_key(key)
    current = root
    for segment in segments[:-1]:
        child = current.get(segment)
        if not isinstance(child, dict):
            raise ValueError(f"Config key not found: {key}")
        current = child
    removed = current.pop(segments[-1], None)
    if removed is None:
        raise ValueError(f"Config key not found: {key}")


def config_help_text():
    return "\n".join(
        [
            "Config commands:",
            "/config help",
            "/config show",
            "/config show <key>",
            "/config get <key>",
            "/config set <key> <value>",
            "/config unset <key>",
        ]
    )


def resolved_config_path(request):
    return expand_home(
        request.get("config_path")
        or os.environ.get("NOODLE_CONFIG")
        or "~/.noodle/config.json"
    )


def resolved_install_root(request):
    override = os.environ.get("NOODLE_INSTALL_ROOT")
    if override:
        return expand_home(override)
    return os.path.dirname(resolved_config_path(request))


def installed_script_path(request, script_name):
    return os.path.join(resolved_install_root(request), "scripts", script_name)


def shell_quote(value):
    return shlex.quote(value)


def update_command(request):
    script_path = installed_script_path(request, "install.sh")
    if os.path.exists(script_path):
        return f"NOODLE_INSTALL_CONFIGURE_LLM=0 zsh {shell_quote(script_path)}"
    return "\n".join(
        [
            "(",
            "  set -e",
            '  tmp="$(mktemp -d "${TMPDIR:-/tmp}/noodle-update.XXXXXX")"',
            '  trap \'rm -rf "$tmp"\' EXIT',
            '  slug="${NOODLE_INSTALL_REPO_SLUG:-alexrudloff/noodle}"',
            '  ref="${NOODLE_INSTALL_REF:-main}"',
            '  archive_url="${NOODLE_INSTALL_ARCHIVE_URL:-https://codeload.github.com/${slug}/tar.gz/${ref}}"',
            '  curl -fsSL "$archive_url" -o "$tmp/noodle.tar.gz"',
            '  tar -xzf "$tmp/noodle.tar.gz" -C "$tmp"',
            '  NOODLE_INSTALL_CONFIGURE_LLM=0 NOODLE_INSTALL_REPO_SLUG="$slug" NOODLE_INSTALL_REF="$ref" NOODLE_INSTALL_ARCHIVE_URL="$archive_url" zsh "$tmp"/*/scripts/install.sh',
            ")",
        ]
    )


def uninstall_command(request):
    script_path = installed_script_path(request, "uninstall.sh")
    if os.path.exists(script_path):
        return f"zsh {shell_quote(script_path)} && exec zsh"
    install_root = shell_quote(resolved_install_root(request))
    return "\n".join(
        [
            "(",
            f"  install_root={install_root}",
            '  launchctl_bin="${NOODLE_LAUNCHCTL_BIN:-launchctl}"',
            '  launch_agent_label="com.noodle.daemon"',
            '  launch_agent_plist="$HOME/Library/LaunchAgents/${launch_agent_label}.plist"',
            '  launchctl_domain="gui/$(id -u)"',
            '  if [[ "${NOODLE_UNINSTALL_YES:-0}" != "1" ]]; then',
            '    if [[ -r /dev/tty && -w /dev/tty ]]; then',
            '      print -n -- "Remove noodle from ${install_root}? [Y/n]: " >/dev/tty',
            '      read -r reply </dev/tty',
            '      case "${reply:l}" in',
            '        ""|y|yes) ;;',
            '        *) print "Cancelled."; exit 1 ;;',
            '      esac',
            "    else",
            '      print -u2 -- "Set NOODLE_UNINSTALL_YES=1 to uninstall when no terminal is available."',
            "      exit 1",
            "    fi",
            "  fi",
            '  "${launchctl_bin}" bootout "${launchctl_domain}/${launch_agent_label}" >/dev/null 2>&1 || true',
            '  "${launchctl_bin}" bootout "${launchctl_domain}" "${launch_agent_plist}" >/dev/null 2>&1 || true',
            '  "${launchctl_bin}" remove "${launch_agent_label}" >/dev/null 2>&1 || true',
            '  python3 - "${ZDOTDIR:-${HOME}}/.zshrc" "${install_root}" <<'"'"'PY'"'"'',
            'from pathlib import Path',
            'import sys',
            '',
            'zshrc_path = Path(sys.argv[1]).expanduser()',
            'install_root = Path(sys.argv[2]).expanduser()',
            'start_marker = "# >>> noodle shell integration >>>"',
            'end_marker = "# <<< noodle shell integration <<<"',
            'legacy_target = str(install_root / "plugin" / "noodle.plugin.zsh")',
            '',
            'if zshrc_path.exists():',
            '    lines = zshrc_path.read_text().splitlines()',
            '    filtered = []',
            '    inside_block = False',
            '    for line in lines:',
            '        stripped = line.strip()',
            '        if stripped == start_marker:',
            '            inside_block = True',
            '            continue',
            '        if stripped == end_marker:',
            '            inside_block = False',
            '            continue',
            '        if inside_block:',
            '            continue',
            '        if "noodle.plugin.zsh" in stripped and stripped.startswith("source "):',
            '            if legacy_target in stripped or "$HOME/.noodle/plugin/noodle.plugin.zsh" in stripped:',
            '                continue',
            '        filtered.append(line)',
            '    while filtered and not filtered[-1].strip():',
            '        filtered.pop()',
            '    zshrc_path.write_text(("\\n".join(filtered) + "\\n") if filtered else "")',
            'PY',
            '  rm -f "${launch_agent_plist}"',
            '  rm -rf "${install_root}"',
            '  print "Removed noodle from ${install_root}."',
            '  print "Removed noodle shell integration from ${ZDOTDIR:-${HOME}}/.zshrc."',
            ") && exec zsh",
        ]
    )


def load_config_document(path):
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def save_config_document(path, value):
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(json.dumps(value, indent=2) + "\n")


def render_help(request):
    lines = [
        "Talk to noodle with: oo <whatever>",
        "Example: oo how do I find every README.md file?",
        "",
        "Slash commands:",
    ]
    for definition in request.get("host", {}).get("slash_commands", []):
        lines.append(f"/{definition['name']} - {definition['description']}")
        lines.append(f"  {definition['usage']}")
    return "\n".join(lines)


def render_status(request):
    config = request.get("config") or {}
    config_path = resolved_config_path(request)
    memory_path = expand_home(
        config.get("memory", {}).get("path")
        or os.environ.get("NOODLE_MEMORY_DB")
        or "~/.noodle/memory.db"
    )
    modules = "\n".join(f"- {item}" for item in request.get("host", {}).get("module_order", []))
    commands = " ".join(
        f"/{item['name']}" for item in request.get("host", {}).get("slash_commands", [])
    )
    chat_prefix = (
        ((config.get("plugins") or {}).get("chat") or {}).get("prefix")
        or os.environ.get("NOODLE_CHAT_PREFIX")
        or ","
    )
    chat_tool_count = (request.get("host", {}).get("tool_counts") or {}).get("chat", 0)
    permission_lines = []
    classes = (((config.get("permissions") or {}).get("classes")) or {})
    for key in [
        "read_only",
        "network_read",
        "local_write",
        "shell_exec",
        "interactive_shell",
        "external",
    ]:
        permission_lines.append(f"- {key}: {classes.get(key, 'unset')}")
    return (
        "Noodle status\n"
        f"Config: {config_path}\n"
        f"Memory DB: {memory_path}\n"
        f"Chat prefix: {chat_prefix}\n"
        f"Chat tools: {chat_tool_count}\n"
        f"Plugins:\n{modules}\n"
        f"Slash commands: {commands}\n"
        f"Permissions:\n" + "\n".join(permission_lines)
    )


def handle_config_command(request, rest):
    config = request.get("config") or {}
    path = resolved_config_path(request)
    if not rest:
        return config_help_text()
    parts = rest.split(None, 1)
    subcommand = parts[0]
    remainder = parts[1].strip() if len(parts) > 1 else ""
    if subcommand == "help":
        return config_help_text()
    if subcommand == "show":
        document = load_config_document(path)
        if remainder:
            value = lookup(document, remainder)
            if value is None:
                raise ValueError(f"Config key not found: {remainder}")
            return json.dumps(value, indent=2)
        return f"Config path: {path}\n" + json.dumps(document, indent=2)
    if subcommand == "get":
        if not remainder:
            raise ValueError("Usage: /config get <key>")
        value = lookup(config, remainder)
        if value is None:
            raise ValueError(f"Config key not found: {remainder}")
        return render_value_inline(value)
    if subcommand == "set":
        split_at = next((i for i, ch in enumerate(remainder) if ch.isspace()), None)
        if split_at is None:
            raise ValueError("Usage: /config set <key> <value>")
        key = remainder[:split_at].strip()
        value_raw = remainder[split_at:].strip()
        if not key or not value_raw:
            raise ValueError("Usage: /config set <key> <value>")
        document = load_config_document(path)
        set_path_value(document, key, parse_config_value(value_raw))
        save_config_document(path, document)
        current = lookup(document, key)
        return f"Updated {key} in {path}.\nNew value: {render_value_inline(current)}"
    if subcommand == "unset":
        if not remainder:
            raise ValueError("Usage: /config unset <key>")
        document = load_config_document(path)
        remove_path_value(document, remainder)
        save_config_document(path, document)
        return f"Removed {remainder} from {path}."
    raise ValueError(f"Unknown config command: {subcommand}.\n{config_help_text()}")


def handle_command(request):
    raw_input = (request.get("input") or "").strip()
    if raw_input == "/help":
        return {
            "action": "message",
            "plugin": UTILS_PLUGIN,
            "message": render_help(request),
        }
    if raw_input == "/status":
        return {
            "action": "message",
            "plugin": UTILS_PLUGIN,
            "message": render_status(request),
        }
    if raw_input == "/reload":
        return {
            "action": "reload_runtime",
            "plugin": UTILS_PLUGIN,
            "message": "Reloaded noodle runtime config.",
        }
    if raw_input.startswith("/config"):
        return {
            "action": "message",
            "plugin": UTILS_PLUGIN,
            "message": handle_config_command(request, raw_input[len("/config") :].strip()),
        }
    if raw_input == "/update":
        return {
            "action": "run",
            "plugin": UTILS_PLUGIN,
            "command": update_command(request),
            "explanation": "Updating noodle in place.",
        }
    if raw_input == "/uninstall":
        return {
            "action": "run",
            "plugin": UTILS_PLUGIN,
            "command": uninstall_command(request),
            "explanation": "Uninstalling noodle after confirmation.",
        }
    raise ValueError(f"Unknown utils command: {raw_input}")


def main():
    request = json.load(sys.stdin)
    stream = bool(request.get("stream"))
    try:
        payload = handle_command(request)
        respond(True, payload=payload, stream=stream)
    except Exception as exc:
        respond(False, error=str(exc), stream=stream)


if __name__ == "__main__":
    main()
