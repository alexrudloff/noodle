# todo plugin

The `todo` plugin provides a small deterministic terminal todo list backed by noodle's shared memory.

## Responsibilities

- create todo items
- list items with stable ids
- mark items open, partial, or done
- remove items
- show a single item
- clear completed items

## Commands

- `/todo list`
- `/todo help`
- `/todo add <task>`
- `/todo / <id>`
  Mark partial
- `/todo x <id>`
  Mark done
- `/todo done <id>`
- `/todo reopen <id>`
  Mark open again
- `/todo remove <id>`
- `/todo rm <id>`
- `/todo show <id>`
- `/todo clear-done`

List output format:

```text
#1 [ ] document plugins
#2 [/] package docs
#3 [x] ship release
```

## Storage

`todo` uses all three shared memory layers:

- `events`
  Command history
- `state`
  Current todo list state
- `artifacts`
  Durable compiled todo list object

## Configuration

Generic plugin block:

- `plugins.todo.uses_tools`
- `plugins.todo.tool_availability`
- `plugins.todo.exports_tools`

Current defaults:

```json
"todo": {
  "uses_tools": [],
  "tool_availability": {},
  "exports_tools": []
}
```

Todo-specific shared memory setting:

- `memory.todo.command_event_limit`
  Maximum number of todo command events retained during compaction and summarization paths

Other related settings:

- `memory.path`
  Shared SQLite database path
- `plugins.order`
  Controls whether `todo` is enabled

## Notes

- `todo` is fully deterministic
- no model call is required for `/todo ...`
- stable numeric ids make it suitable for muscle-memory terminal workflows
