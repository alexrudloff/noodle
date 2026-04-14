#!/usr/bin/env python3
import json
import os
import sqlite3
import sys
import time

TODO_PLUGIN = "todo"
TODO_STATE_KEY = "items"
TODO_ARTIFACT_KIND = "list"


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
        CREATE TABLE IF NOT EXISTS artifacts (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          plugin TEXT NOT NULL,
          kind TEXT NOT NULL,
          content TEXT NOT NULL,
          source_json TEXT NOT NULL DEFAULT '{}',
          active INTEGER NOT NULL DEFAULT 1,
          created_at INTEGER NOT NULL DEFAULT (unixepoch()),
          updated_at INTEGER NOT NULL DEFAULT (unixepoch())
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
        CREATE INDEX IF NOT EXISTS idx_artifacts_plugin_kind_active
          ON artifacts(plugin, kind, active, updated_at DESC, id DESC)
        """
    )
    return conn


def unix_timestamp():
    return int(time.time())


def todo_help_text():
    return "\n".join(
        [
            "Todo commands:",
            "/todo list",
            "/todo add <task>",
            "/todo / <id>",
            "/todo x <id>",
            "/todo done <id>",
            "/todo reopen <id>",
            "/todo remove <id>",
            "/todo show <id>",
            "/todo clear-done",
        ]
    )


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
    current = load_json_state(conn, plugin, key, 0)
    next_value = int(current) + 1
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


def upsert_artifact(conn, plugin, kind, content, source):
    conn.execute(
        "UPDATE artifacts SET active = 0 WHERE plugin = ?1 AND kind = ?2 AND active = 1",
        (plugin, kind),
    )
    conn.execute(
        """
        INSERT INTO artifacts(plugin, kind, content, source_json, active, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, 1, unixepoch(), unixepoch())
        """,
        (plugin, kind, content, json.dumps(source)),
    )


def todo_event_limit(config):
    return (
        config.get("memory", {})
        .get("todo", {})
        .get("command_event_limit", 200)
    )


def load_todos(conn):
    return load_json_state(conn, TODO_PLUGIN, TODO_STATE_KEY, [])


def render_todo_list(items):
    if not items:
        return "No todos yet.\nUse /todo add <task> to create one."

    lines = []
    open_count = len([item for item in items if not item["done"] and not item.get("partial", False)])
    partial_count = len([item for item in items if item.get("partial", False) and not item["done"]])
    done_count = len([item for item in items if item["done"]])
    lines.append(f"Todos: {open_count} open, {partial_count} partial, {done_count} done")
    for item in items:
        if not item["done"] and not item.get("partial", False):
            lines.append(f"#{item['id']} [ ] {item['text']}")
    for item in items:
        if item.get("partial", False) and not item["done"]:
            lines.append(f"#{item['id']} [/] {item['text']}")
    for item in items:
        if item["done"]:
            lines.append(f"#{item['id']} [x] {item['text']}")
    return "\n".join(lines)


def render_todo_detail(item):
    status = "done" if item["done"] else "partial" if item.get("partial", False) else "open"
    lines = [
        f"Todo #{item['id']}",
        f"Status: {status}",
        f"Task: {item['text']}",
        f"Created: {item['created_at']}",
        f"Updated: {item['updated_at']}",
    ]
    if item.get("completed_at") is not None:
        lines.append(f"Completed: {item['completed_at']}")
    return "\n".join(lines)


def save_todos(conn, items):
    set_json_state(conn, TODO_PLUGIN, TODO_STATE_KEY, items)
    summary = render_todo_list(items)
    upsert_artifact(
        conn,
        TODO_PLUGIN,
        TODO_ARTIFACT_KIND,
        summary,
        {
            "open": len([item for item in items if not item["done"]]),
            "done": len([item for item in items if item["done"]]),
            "count": len(items),
        },
    )


def record_todo_event(conn, config, key, value):
    append_event(conn, TODO_PLUGIN, "command", key, value)
    trim_events(conn, TODO_PLUGIN, "command", int(todo_event_limit(config)))


def parse_id(remainder, name):
    try:
        return int(remainder)
    except ValueError:
        raise ValueError(f"Usage: /todo {name} <id>")


def find_item(items, todo_id):
    for item in items:
        if item["id"] == todo_id:
            return item
    raise ValueError(f"Todo #{todo_id} does not exist.")


def handle_command(config, raw_input):
    trimmed = raw_input.strip()
    if not trimmed.startswith("/todo"):
        raise ValueError("todo commands must start with /todo")
    rest = trimmed[len("/todo") :].strip()
    if not rest:
        conn = memory_connection(config)
        return render_todo_list(load_todos(conn))

    parts = rest.split(None, 1)
    subcommand = parts[0]
    remainder = parts[1].strip() if len(parts) > 1 else ""
    conn = memory_connection(config)

    if subcommand == "help":
        return todo_help_text()
    if subcommand == "list":
        return render_todo_list(load_todos(conn))
    if subcommand == "add":
        if not remainder:
            raise ValueError("Usage: /todo add <task>")
        items = load_todos(conn)
        todo_id = increment_counter(conn, TODO_PLUGIN, "next_id")
        now = unix_timestamp()
        item = {
            "id": todo_id,
            "text": remainder,
            "partial": False,
            "done": False,
            "created_at": now,
            "updated_at": now,
            "completed_at": None,
        }
        items.append(item)
        save_todos(conn, items)
        record_todo_event(conn, config, "add", {"id": todo_id, "text": remainder, "done": False})
        conn.commit()
        return f"Added todo #{todo_id}: {remainder}"
    if subcommand in {"/", "partial"}:
        todo_id = parse_id(remainder, "partial")
        items = load_todos(conn)
        item = find_item(items, todo_id)
        if item.get("partial", False):
            return f"Todo #{item['id']} is already partially done."
        item["partial"] = True
        item["done"] = False
        item["updated_at"] = unix_timestamp()
        item["completed_at"] = None
        save_todos(conn, items)
        record_todo_event(conn, config, "partial", {"id": todo_id, "text": item["text"], "partial": True, "done": False})
        conn.commit()
        return f"Marked todo #{todo_id} as partial: {item['text']}"
    if subcommand in {"x", "done"}:
        todo_id = parse_id(remainder, subcommand)
        items = load_todos(conn)
        item = find_item(items, todo_id)
        if item["done"]:
            return f"Todo #{item['id']} is already done."
        now = unix_timestamp()
        item["partial"] = False
        item["done"] = True
        item["updated_at"] = now
        item["completed_at"] = now
        save_todos(conn, items)
        record_todo_event(conn, config, "done", {"id": todo_id, "text": item["text"], "partial": False, "done": True})
        conn.commit()
        return f"Completed todo #{todo_id}: {item['text']}"
    if subcommand == "reopen":
        todo_id = parse_id(remainder, "reopen")
        items = load_todos(conn)
        item = find_item(items, todo_id)
        if not item["done"] and not item.get("partial", False):
            return f"Todo #{item['id']} is already open."
        item["partial"] = False
        item["done"] = False
        item["updated_at"] = unix_timestamp()
        item["completed_at"] = None
        save_todos(conn, items)
        record_todo_event(conn, config, "reopen", {"id": todo_id, "text": item["text"], "partial": False, "done": False})
        conn.commit()
        return f"Reopened todo #{todo_id}: {item['text']}"
    if subcommand in {"remove", "rm"}:
        todo_id = parse_id(remainder, "remove")
        items = load_todos(conn)
        index = next((i for i, item in enumerate(items) if item["id"] == todo_id), None)
        if index is None:
            raise ValueError(f"Todo #{todo_id} does not exist.")
        item = items.pop(index)
        save_todos(conn, items)
        record_todo_event(conn, config, "remove", {"id": todo_id, "text": item["text"], "done": item["done"]})
        conn.commit()
        return f"Removed todo #{item['id']}: {item['text']}"
    if subcommand == "show":
        todo_id = parse_id(remainder, "show")
        item = find_item(load_todos(conn), todo_id)
        return render_todo_detail(item)
    if subcommand == "clear-done":
        items = load_todos(conn)
        removed = len([item for item in items if item["done"]])
        if removed == 0:
            return "No completed todos to clear."
        items = [item for item in items if not item["done"]]
        save_todos(conn, items)
        record_todo_event(conn, config, "clear_done", {"removed": removed})
        conn.commit()
        return f"Cleared {removed} completed todo(s)."
    raise ValueError(f"Unknown todo command: {subcommand}.\n{todo_help_text()}")


def main():
    request = json.load(sys.stdin)
    config = request.get("config") or {}
    stream = bool(request.get("stream"))
    try:
        message = handle_command(config, request.get("input", ""))
        respond(
            True,
            {
                "action": "message",
                "plugin": TODO_PLUGIN,
                "message": message,
            },
            stream=stream,
        )
    except Exception as exc:
        respond(False, error=str(exc), stream=stream)


if __name__ == "__main__":
    main()
