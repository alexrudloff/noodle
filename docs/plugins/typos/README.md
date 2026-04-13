# typos plugin

The `typos` plugin handles typo recovery for shell commands.

## Responsibilities

- react to `command not found`
- optionally react to general command failures when error fallback is enabled
- propose likely intended commands
- either present a selection menu or auto-run the top correction

## Inputs

Primary input:

- `command_not_found`

Optional input:

- `command_error`

`command_error` forwarding is only active when `runtime.enable_error_fallback` is enabled in config or via environment override.

## Behavior

The plugin asks the configured model for candidate shell commands using the typo prompt, then:

- shows a menu when `selection_mode` is `select`
- auto-runs the top choice when `selection_mode` is `auto`

## Configuration

### Plugin block

- `plugins.typos.selection_mode`
  Supported values: `select`, `auto`
- `plugins.typos.uses_tools`
- `plugins.typos.tool_availability`
- `plugins.typos.exports_tools`
- `plugins.typos.prompt`
  Prompt template used for typo recovery

Current default shape:

```json
"typos": {
  "selection_mode": "select",
  "uses_tools": [],
  "exports_tools": [],
  "prompt": "You are a zsh typo fixer..."
}
```

### Memory settings

- `memory.typos.context_limit`
  Number of prior typo-related items included as context
- `memory.typos.selection_event_limit`
  Retained selection history size for typo flows

### Related global settings

- `runtime.auto_run`
  Affects whether selected commands are executed automatically
- `runtime.enable_error_fallback`
  Enables the `command_error` event path
- `runtime.max_retry_depth`
  Prevents recursive correction loops

## Environment Overrides

Relevant overrides for `typos`:

- `NOODLE_PROMPT`
- `NOODLE_SELECTION_MODE`
- `NOODLE_AUTO_RUN`
- `NOODLE_ENABLE_ERROR_FALLBACK`
- `NOODLE_MAX_RETRY_DEPTH`

## Notes

- `typos` is model-assisted, but the selection and execution path is deterministic once candidates are returned
- it is intentionally narrow: typo correction, not general shell chat
