#!/usr/bin/env python3
import json
import os
import sqlite3
import sys

MEMORY_PLUGIN = "memory"


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
        CREATE INDEX IF NOT EXISTS idx_artifacts_plugin_kind_active
          ON artifacts(plugin, kind, active, updated_at DESC, id DESC)
        """
    )
    return conn


def trim_one_line(text, limit):
    flattened = text.replace("\n", " ")
    if len(flattened) <= limit:
        return flattened
    return flattened[:limit] + "..."


def render_search_entry(key, value):
    if not key.strip():
        return trim_one_line(value, 120)
    return f"{key} {trim_one_line(value, 100)}"


def memory_help_text():
    return "\n".join(
        [
            "Memory commands:",
            "/memory",
            "/memory help",
            "/memory search <term>",
            "/memory clear <plugin|all>",
        ]
    )


def render_summary(config):
    conn = memory_connection(config)
    events = conn.execute("SELECT COUNT(*) FROM events").fetchone()[0]
    state = conn.execute("SELECT COUNT(*) FROM state").fetchone()[0]
    artifacts_total = conn.execute("SELECT COUNT(*) FROM artifacts").fetchone()[0]
    artifacts_active = conn.execute(
        "SELECT COUNT(*) FROM artifacts WHERE active = 1"
    ).fetchone()[0]
    rows = conn.execute(
        """
        SELECT plugin,
               (SELECT COUNT(*) FROM events e WHERE e.plugin = p.plugin) AS event_count,
               (SELECT COUNT(*) FROM state s WHERE s.plugin = p.plugin) AS state_count,
               (SELECT COUNT(*) FROM artifacts a WHERE a.plugin = p.plugin AND a.active = 1) AS active_artifacts
          FROM (
            SELECT plugin FROM events
            UNION
            SELECT plugin FROM state
            UNION
            SELECT plugin FROM artifacts
          ) p
         ORDER BY plugin
        """
    ).fetchall()

    lines = [
        f"Memory DB: {memory_path(config)}",
        f"Events: {events}",
        f"State keys: {state}",
        f"Artifacts: {artifacts_active} active / {artifacts_total} total",
    ]
    if rows:
        lines.append("Plugins:")
        for plugin, event_count, state_count, active_artifacts in rows:
            lines.append(
                f"- {plugin}: {event_count} events, {state_count} state keys, {active_artifacts} active artifacts"
            )
    return "\n".join(lines)


def render_search(config, term):
    conn = memory_connection(config)
    pattern = f"%{term.lower()}%"
    lines = []
    for plugin, kind, key, value_json in conn.execute(
        """
        SELECT plugin, kind, key, value_json
          FROM events
         WHERE lower(key) LIKE ?1 OR lower(value_json) LIKE ?1
         ORDER BY created_at DESC, id DESC
         LIMIT 5
        """,
        (pattern,),
    ).fetchall():
        lines.append(f"[event] {plugin}.{kind} {render_search_entry(key, value_json)}")
    for plugin, key, value_json in conn.execute(
        """
        SELECT plugin, key, value_json
          FROM state
         WHERE lower(key) LIKE ?1 OR lower(value_json) LIKE ?1
         ORDER BY updated_at DESC
         LIMIT 5
        """,
        (pattern,),
    ).fetchall():
        lines.append(f"[state] {plugin}.{key} {render_search_entry(key, value_json)}")
    for plugin, kind, content in conn.execute(
        """
        SELECT plugin, kind, content
          FROM artifacts
         WHERE lower(kind) LIKE ?1 OR lower(content) LIKE ?1 OR lower(source_json) LIKE ?1
         ORDER BY updated_at DESC, id DESC
         LIMIT 5
        """,
        (pattern,),
    ).fetchall():
        lines.append(f"[artifact] {plugin}.{kind} {trim_one_line(content, 120)}")
    if not lines:
        return f'No memory matches for "{term}".'
    return f"Memory search: {term}\n" + "\n".join(lines)


def clear_scope(config, scope):
    conn = memory_connection(config)
    if scope == "all":
        events_deleted = conn.execute("DELETE FROM events").rowcount
        state_deleted = conn.execute("DELETE FROM state").rowcount
        artifacts_deleted = conn.execute("DELETE FROM artifacts").rowcount
    else:
        events_deleted = conn.execute(
            "DELETE FROM events WHERE plugin = ?1", (scope,)
        ).rowcount
        state_deleted = conn.execute(
            "DELETE FROM state WHERE plugin = ?1", (scope,)
        ).rowcount
        artifacts_deleted = conn.execute(
            "DELETE FROM artifacts WHERE plugin = ?1", (scope,)
        ).rowcount
    conn.commit()
    return (
        f"Cleared memory for {scope}: "
        f"{events_deleted} events, {state_deleted} state keys, {artifacts_deleted} artifacts."
    )


def handle_command(config, raw_input):
    trimmed = raw_input.strip()
    if not trimmed.startswith("/memory"):
        raise ValueError("memory commands must start with /memory")
    rest = trimmed[len("/memory") :].strip()
    if not rest:
        return render_summary(config)
    parts = rest.split(None, 1)
    subcommand = parts[0]
    remainder = parts[1].strip() if len(parts) > 1 else ""
    if subcommand == "help":
        return memory_help_text()
    if subcommand == "search":
        if not remainder:
            raise ValueError("Usage: /memory search <term>")
        return render_search(config, remainder)
    if subcommand == "clear":
        if not remainder:
            raise ValueError("Usage: /memory clear <plugin|all>")
        return clear_scope(config, remainder)
    raise ValueError(f"Unknown memory command: {subcommand}.\n{memory_help_text()}")


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
                "plugin": MEMORY_PLUGIN,
                "message": message,
            },
            stream=stream,
        )
    except Exception as exc:
        respond(False, error=str(exc), stream=stream)


if __name__ == "__main__":
    main()
