#!/usr/bin/env python3
import json
import os
import re
import sqlite3
import subprocess
import sys

TYPOS_PLUGIN = "typos"
DEFAULT_TYPOS_PROMPT = """You are a zsh typo fixer.
The user typed a mistaken command and zsh could not find it.
Return exactly 3 lines.
Each line must contain only one command the user most likely intended to run in zsh.
Prefer common shell commands over obscure executables.
Prefer the intended zsh command, not merely the nearest executable name.
No numbering.
No explanation.
No extra text.
Input: {user_input}
"""


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


def memory_path(config):
    raw = (
        config.get("memory", {}).get("path")
        or os.environ.get("NOODLE_MEMORY_DB")
        or "~/.noodle/memory.db"
    )
    return expand_home(raw)


def memory_connection(config):
    path = memory_path(config)
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    conn = sqlite3.connect(path)
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS events (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          plugin TEXT NOT NULL,
          kind TEXT NOT NULL,
          key TEXT NOT NULL DEFAULT '',
          value_json TEXT NOT NULL,
          created_at INTEGER NOT NULL DEFAULT (unixepoch())
        )
        """
    )
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS state (
          plugin TEXT NOT NULL,
          key TEXT NOT NULL,
          value_json TEXT NOT NULL,
          updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
          PRIMARY KEY(plugin, key)
        )
        """
    )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_events_plugin_kind_created
          ON events(plugin, kind, created_at DESC, id DESC)
        """
    )
    conn.execute(
        """
        CREATE INDEX IF NOT EXISTS idx_events_plugin_key_created
          ON events(plugin, key, created_at DESC)
        """
    )
    return conn


def load_json_state(conn, plugin, key, default):
    row = conn.execute(
        "SELECT value_json FROM state WHERE plugin = ?1 AND key = ?2",
        (plugin, key),
    ).fetchone()
    if row is None:
        return default
    return json.loads(row[0])


def set_json_state(conn, plugin, key, value):
    conn.execute(
        """
        INSERT INTO state(plugin, key, value_json, updated_at)
        VALUES (?1, ?2, ?3, unixepoch())
        ON CONFLICT(plugin, key)
        DO UPDATE SET value_json = excluded.value_json, updated_at = unixepoch()
        """,
        (plugin, key, json.dumps(value)),
    )


def increment_counter(conn, plugin, key):
    next_value = int(load_json_state(conn, plugin, key, 0)) + 1
    set_json_state(conn, plugin, key, next_value)
    return next_value


def append_event(conn, plugin, kind, key, value):
    conn.execute(
        "INSERT INTO events(plugin, kind, key, value_json) VALUES (?1, ?2, ?3, ?4)",
        (plugin, kind, key, json.dumps(value)),
    )


def trim_events(conn, plugin, kind, keep):
    conn.execute(
        """
        DELETE FROM events
         WHERE plugin = ?1 AND kind = ?2
           AND id NOT IN (
             SELECT id FROM events
              WHERE plugin = ?1 AND kind = ?2
              ORDER BY created_at DESC, id DESC
              LIMIT ?3
           )
        """,
        (plugin, kind, keep),
    )


def typo_context_limit(config):
    return max(1, int(config.get("memory", {}).get("typos", {}).get("context_limit", 3)))


def selection_event_limit(config):
    return max(
        10,
        int(config.get("memory", {}).get("typos", {}).get("selection_event_limit", 200)),
    )


def typo_context(config, user_input):
    conn = memory_connection(config)
    prefix = f"{user_input}\t"
    pattern = f"{prefix}%"
    rows = conn.execute(
        """
        SELECT key, value_json
          FROM state
         WHERE plugin = ?1 AND key LIKE ?2
         ORDER BY updated_at DESC
        """,
        (TYPOS_PLUGIN, pattern),
    ).fetchall()
    results = []
    for key, raw in rows:
        selected = key[len(prefix) :] if key.startswith(prefix) else key
        try:
            count = int(json.loads(raw))
        except Exception:
            count = 0
        results.append((selected, count))
    results.sort(key=lambda item: (-item[1], item[0]))
    return results[: typo_context_limit(config)]


def typo_template(config):
    return (
        os.environ.get("NOODLE_PROMPT")
        or ((config.get("plugins") or {}).get("typos") or {}).get("prompt")
        or DEFAULT_TYPOS_PROMPT
    )


def render_sections(sections):
    rendered = []
    for title, body in sections:
        body = body.strip()
        if body:
            rendered.append(f"[{title}]\n{body}")
    return "\n\n".join(rendered)


def build_prompt(request, extra_sections):
    template = typo_template(request.get("config") or {})
    prompt = (
        template.replace("{mode}", "command_not_found")
        .replace("{cwd}", request.get("cwd") or "")
        .replace("{shell}", request.get("shell") or "")
        .replace("{exit_status}", str(request.get("exit_status") or 0))
        .replace("{recent_command}", request.get("recent_command") or "")
        .replace("{user_input}", request.get("input") or "")
        .strip()
    )
    sections = [("Operating Instructions", prompt)]
    extra = "\n\n".join(section.strip() for section in extra_sections if section.strip())
    if extra:
        sections.append(("Additional Context", extra))
    return render_sections(sections)


def clean_response_text(text):
    cleaned = text.strip().replace("```json", "").replace("```", "")
    for pattern in ("<think>", "</think>", "<thinking>", "</thinking>"):
        cleaned = cleaned.replace(pattern, "")
    lines = []
    for line in cleaned.splitlines():
        line = line.strip()
        if not line:
            continue
        if line.lower() in {"json:", "command:", "answer:"}:
            continue
        lines.append(line)
    return "\n".join(lines)


def normalize_line(line):
    line = line.strip().strip("`").strip("\"'").strip()
    return re.sub(r"^[\d\.\):\s]+", "", line).strip()


def dedupe(items):
    seen = set()
    result = []
    for item in items:
        if item and item not in seen:
            seen.add(item)
            result.append(item)
    return result


def infer_payload(text):
    if not text.strip():
        raise ValueError("empty model response")
    lines = [normalize_line(line) for line in text.splitlines()]
    lines = [line for line in lines if line]
    if not lines:
        raise ValueError("empty model response")
    if len(lines) > 1:
        return {"action": "select", "choices": dedupe(lines[:3]), "plugin": TYPOS_PLUGIN}
    line = lines[0]
    if line.endswith("?"):
        return {"action": "ask", "question": line, "plugin": TYPOS_PLUGIN}
    return {
        "action": "run",
        "command": line,
        "explanation": "",
        "plugin": TYPOS_PLUGIN,
    }


def host_binary(request):
    return (
        ((request.get("host") or {}).get("binary_path"))
        or os.environ.get("NOODLE_HELPER")
        or "noodle"
    )


def module_api_prefix(request):
    module_api = ((request.get("host") or {}).get("module_api")) or {}
    prefix = module_api.get("command_prefix")
    if isinstance(prefix, list) and prefix:
        return prefix
    return [host_binary(request), "module-api"]


def resolved_config_path(request):
    return expand_home(
        request.get("config_path")
        or os.environ.get("NOODLE_CONFIG")
        or "~/.noodle/config.json"
    )


def model_output(request, prompt):
    command = module_api_prefix(request) + [
        "model-output",
        "--config",
        resolved_config_path(request),
    ]
    if request.get("debug"):
        command.append("--debug")
    env = os.environ.copy()
    env["NOODLE_BYPASS_DAEMON"] = "1"
    result = subprocess.run(
        command,
        input=prompt,
        text=True,
        capture_output=True,
        env=env,
        check=False,
    )
    if result.returncode == 0:
        return result.stdout
    detail = result.stderr.strip() or result.stdout.strip() or "model-output failed"
    raise RuntimeError(detail)


def handle_typo_suggestions(request):
    config = request.get("config") or {}
    user_input = (request.get("input") or "").strip()
    typo_history = typo_context(config, user_input)
    extra_sections = []
    if typo_history:
        lines = "\n".join(
            f"- {choice} ({count})" for choice, count in typo_history if choice.strip()
        )
        if lines:
            extra_sections.append(
                "Past selected corrections for this exact input:\n" + lines
            )
    prompt = build_prompt(request, extra_sections)
    output = model_output(request, prompt)
    return infer_payload(clean_response_text(output))


def handle_typo_selected(request):
    config = request.get("config") or {}
    user_input = (request.get("input") or "").strip()
    selected_command = (request.get("selected_command") or "").strip()
    if not user_input or not selected_command:
        return {"action": "noop", "plugin": TYPOS_PLUGIN}
    conn = memory_connection(config)
    append_event(
        conn,
        TYPOS_PLUGIN,
        "selection",
        user_input,
        {"input": user_input, "selected": selected_command},
    )
    increment_counter(conn, TYPOS_PLUGIN, f"{user_input}\t{selected_command}")
    trim_events(conn, TYPOS_PLUGIN, "selection", selection_event_limit(config))
    conn.commit()
    return {"action": "noop", "plugin": TYPOS_PLUGIN}


def handle_request(request):
    event = request.get("event")
    if event in {"command_not_found", "command_error"}:
        return handle_typo_suggestions(request)
    if event == "typo_selected":
        return handle_typo_selected(request)
    raise ValueError(f"unsupported typos event: {event}")


def main():
    request = json.load(sys.stdin)
    stream = bool(request.get("stream"))
    try:
        respond(True, handle_request(request), stream=stream)
    except Exception as exc:
        respond(False, error=str(exc), stream=stream)


if __name__ == "__main__":
    main()
