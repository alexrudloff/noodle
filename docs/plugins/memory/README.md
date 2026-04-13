# memory plugin

The `memory` plugin exposes deterministic inspection and maintenance commands for noodle's shared SQLite memory.

## Responsibilities

- summarize current memory usage
- search memory across events, state, and artifacts
- clear memory by plugin or clear all memory

This plugin does not own a separate storage system. It operates on the shared daemon memory layer.

## Commands

- `/memory`
  Shows memory DB path, total counts, and per-plugin usage summary
- `/memory help`
- `/memory search <term>`
  Searches:
  - event keys and values
  - state keys and values
  - artifact kinds, contents, and source metadata
- `/memory clear <plugin|all>`

## Shared Memory Model

The daemon uses three shared tables:

- `events`
  Immutable event log
- `state`
  Derived key/value state
- `artifacts`
  Compiled durable objects such as tasks, summaries, and handoffs

`/memory clear <plugin>` removes that plugin's rows from all three layers.

## Configuration

The `memory` plugin has no dedicated plugin-only knobs beyond the generic plugin block:

- `plugins.memory.uses_tools`
- `plugins.memory.tool_availability`
- `plugins.memory.exports_tools`

Current defaults:

```json
"memory": {
  "uses_tools": [],
  "tool_availability": {},
  "exports_tools": []
}
```

Related shared settings:

- `memory.path`
  Location of the SQLite database
- `plugins.order`
  Controls whether the `memory` plugin is enabled
- `NOODLE_MEMORY_DB`
  Environment override for the DB path

## Notes

- `memory` is deterministic by design
- no model call is required for `/memory ...`
- the plugin is an operator view over shared daemon state, not a second memory implementation
