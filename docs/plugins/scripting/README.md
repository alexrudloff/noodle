# scripting plugin

The `scripting` plugin exposes deterministic shell-scripting primitives backed by noodle's shared SQLite memory.

## Responsibilities

- provide small, composable scripting commands that do not require a model call
- expose shared state primitives that work across shells and daemon sessions
- keep scripting-oriented state separate from the operator-focused `/memory` surface

Today the plugin provides a shared KV cache. Future deterministic primitives can live here too.

## Commands

- `/kv help`
- `/kv get <key>`
  Returns the stored string value for a key if present and not expired
- `/kv set <key> <value> [--ttl <duration>]`
  Stores a string value in shared state for shell-friendly caching
- `/kv unset <key>`
  Removes a stored key/value entry

TTL durations accept seconds by default or `s`, `m`, `h`, and `d` suffixes.
Examples: `30`, `30s`, `5m`, `2h`, `1d`

## Storage Model

The `scripting` plugin uses the shared `state` table in noodle's memory DB.
KV entries are namespaced under the `scripting` plugin and stored with internal `kv:` key prefixes.

Expired entries are swept lazily on the next `/kv ...` command.

## Configuration

The `scripting` plugin currently has no plugin-specific knobs beyond the generic plugin block:

- `plugins.scripting.uses_tools`
- `plugins.scripting.tool_availability`
- `plugins.scripting.exports_tools`

Current defaults:

```json
"scripting": {
  "uses_tools": [],
  "tool_availability": {},
  "exports_tools": []
}
```

Related shared settings:

- `plugins.order`
  Controls whether the `scripting` plugin is enabled
- `memory.path`
  Location of the shared SQLite database
- `NOODLE_MEMORY_DB`
  Environment override for the DB path
