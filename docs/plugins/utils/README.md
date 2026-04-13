# utils plugin

The `utils` plugin provides deterministic shell utilities that do not require the model.

## Responsibilities

- expose `/help`
- expose `/status`
- expose `/reload`
- expose `/config ...`

This plugin is the operator/control surface for noodle itself.

## Commands

- `/help`
  Lists registered slash commands and short usage text
- `/status`
  Shows active config path, memory DB path, plugin order, slash commands, chat prefix, chat tool count, and permission classes
- `/reload`
  Reloads cached runtime config in the current shell
- `/config help`
- `/config show`
- `/config show <key>`
- `/config get <key>`
- `/config set <key> <value>`
- `/config unset <key>`

`/config` edits the current config file, not just in-memory state.

## Configuration

The `utils` plugin has no dedicated runtime knobs beyond the generic plugin block:

- `plugins.utils.uses_tools`
- `plugins.utils.tool_availability`
- `plugins.utils.exports_tools`

Current defaults:

```json
"utils": {
  "uses_tools": [],
  "tool_availability": {},
  "exports_tools": []
}
```

Related settings outside the plugin block:

- `plugins.order`
  Controls whether `utils` is enabled
- `runtime.*`
  `/reload` refreshes these cached values in the shell adapter
- `permissions.*`
  `/status` displays the current permission policy
- `NOODLE_CONFIG`
  Controls which config file `/config` edits

## Notes

- `utils` is deterministic by design
- no model call is required for any `utils` slash command
- `/reload` reloads shell-side cached runtime values without requiring a fresh shell session
