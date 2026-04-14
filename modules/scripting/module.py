#!/usr/bin/env python3
import json
import os
import sqlite3
import sys
import time

SCRIPTING_PLUGIN = "scripting"
KV_PREFIX = "kv:"


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
    return conn


def unix_now():
    return time.time()


def kv_state_key(key):
    return f"{KV_PREFIX}{key}"


def kv_help_text():
    return "\n".join(
        [
            "Scripting commands:",
            "/kv help",
            "/kv get <key>",
            "/kv set <key> <value> [--ttl <duration>]",
            "/kv unset <key>",
            "TTL durations accept seconds by default or s/m/h/d suffixes.",
        ]
    )


def token_spans(text):
    spans = []
    start = None
    for index, ch in enumerate(text):
        if ch.isspace():
            if start is not None:
                spans.append((start, text[start:index]))
                start = None
        elif start is None:
            start = index
    if start is not None:
        spans.append((start, text[start:]))
    return spans


def parse_ttl_seconds(raw):
    value = raw.strip()
    if not value:
        raise ValueError("TTL duration cannot be empty.")
    suffix = value[-1]
    if suffix in {"s", "m", "h", "d"}:
        number = value[:-1]
        multiplier = {"s": 1, "m": 60, "h": 60 * 60, "d": 60 * 60 * 24}[suffix]
    else:
        number = value
        multiplier = 1
    try:
        seconds = int(number)
    except ValueError:
        raise ValueError(f"Invalid TTL duration: {value}")
    if seconds <= 0:
        raise ValueError("TTL duration must be greater than zero.")
    return seconds * multiplier


def strip_trailing_ttl(text):
    spans = token_spans(text)
    if spans:
        _, last = spans[-1]
        if last.startswith("--ttl="):
            ttl_seconds = parse_ttl_seconds(last.split("=", 1)[1])
            body = text[: spans[-1][0]].rstrip()
            if not body:
                raise ValueError("Usage: /kv set <key> <value> [--ttl <duration>]")
            return body, ttl_seconds
    if len(spans) >= 2 and spans[-2][1] == "--ttl":
        ttl_seconds = parse_ttl_seconds(spans[-1][1])
        body = text[: spans[-2][0]].rstrip()
        if not body:
            raise ValueError("Usage: /kv set <key> <value> [--ttl <duration>]")
        return body, ttl_seconds
    return text, None


def parse_set_args(rest):
    value = rest.strip()
    if not value:
        raise ValueError("Usage: /kv set <key> <value> [--ttl <duration>]")
    body, ttl_seconds = strip_trailing_ttl(value)
    first_space = next((i for i, ch in enumerate(body) if ch.isspace()), None)
    if first_space is None:
        raise ValueError("Usage: /kv set <key> <value> [--ttl <duration>]")
    key = body[:first_space].strip()
    body_value = body[first_space:].strip()
    if not key or not body_value:
        raise ValueError("Usage: /kv set <key> <value> [--ttl <duration>]")
    return key, body_value, ttl_seconds


def format_ttl_seconds(ttl_seconds):
    if ttl_seconds % (60 * 60 * 24) == 0:
        return f"{ttl_seconds // (60 * 60 * 24)}d"
    if ttl_seconds % (60 * 60) == 0:
        return f"{ttl_seconds // (60 * 60)}h"
    if ttl_seconds % 60 == 0:
        return f"{ttl_seconds // 60}m"
    return f"{ttl_seconds}s"


def load_entry(conn, key):
    row = conn.execute(
        "SELECT value_json FROM state WHERE plugin = ?1 AND key = ?2",
        (SCRIPTING_PLUGIN, kv_state_key(key)),
    ).fetchone()
    if row is None:
        return None
    entry = json.loads(row[0])
    expires_at = entry.get("expires_at")
    if expires_at is not None and expires_at <= unix_now():
        conn.execute(
            "DELETE FROM state WHERE plugin = ?1 AND key = ?2",
            (SCRIPTING_PLUGIN, kv_state_key(key)),
        )
        conn.commit()
        return None
    return entry


def purge_expired(conn):
    rows = conn.execute(
        "SELECT key, value_json FROM state WHERE plugin = ?1 AND key LIKE ?2",
        (SCRIPTING_PLUGIN, f"{KV_PREFIX}%"),
    ).fetchall()
    now = unix_now()
    expired = []
    for key, raw_value in rows:
        try:
            entry = json.loads(raw_value)
        except json.JSONDecodeError:
            continue
        expires_at = entry.get("expires_at")
        if expires_at is not None and expires_at <= now:
            expired.append(key)
    for key in expired:
        conn.execute(
            "DELETE FROM state WHERE plugin = ?1 AND key = ?2",
            (SCRIPTING_PLUGIN, key),
        )
    if expired:
        conn.commit()


def handle_command(config, raw_input):
    conn = memory_connection(config)
    purge_expired(conn)
    trimmed = raw_input.strip()
    if not trimmed.startswith("/kv"):
        raise ValueError("scripting kv commands must start with /kv")
    rest = trimmed[len("/kv") :].strip()
    if not rest:
        return kv_help_text()
    parts = rest.split(None, 1)
    subcommand = parts[0]
    remainder = parts[1].strip() if len(parts) > 1 else ""

    if subcommand == "help":
        return kv_help_text()
    if subcommand == "get":
        if not remainder:
            raise ValueError("Usage: /kv get <key>")
        entry = load_entry(conn, remainder)
        return entry["value"] if entry is not None else f"KV key not found: {remainder}"
    if subcommand == "set":
        key, value, ttl_seconds = parse_set_args(remainder)
        now = unix_now()
        entry = {
            "value": value,
            "created_at": now,
            "expires_at": None if ttl_seconds is None else now + ttl_seconds,
        }
        conn.execute(
            """
            INSERT INTO state(plugin, key, value_json, updated_at)
            VALUES (?1, ?2, ?3, unixepoch())
            ON CONFLICT(plugin, key)
            DO UPDATE SET value_json = excluded.value_json, updated_at = unixepoch()
            """,
            (SCRIPTING_PLUGIN, kv_state_key(key), json.dumps(entry)),
        )
        conn.commit()
        ttl_suffix = "" if ttl_seconds is None else f" TTL {format_ttl_seconds(ttl_seconds)}"
        return f"Set kv key {key}.{ttl_suffix}"
    if subcommand == "unset":
        if not remainder:
            raise ValueError("Usage: /kv unset <key>")
        deleted = conn.execute(
            "DELETE FROM state WHERE plugin = ?1 AND key = ?2",
            (SCRIPTING_PLUGIN, kv_state_key(remainder)),
        ).rowcount
        conn.commit()
        if deleted:
            return f"Removed kv key {remainder}."
        return f"KV key was not set: {remainder}"
    raise ValueError(f"Unknown kv command: {subcommand}.\n{kv_help_text()}")


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
                "plugin": SCRIPTING_PLUGIN,
                "message": message,
            },
            stream=stream,
        )
    except Exception as exc:
        respond(False, error=str(exc), stream=stream)


if __name__ == "__main__":
    main()
