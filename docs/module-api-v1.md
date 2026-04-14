# Module API v1

`noodle` exposes an explicit versioned host contract for external modules.
Third-party modules should target `api_version: "v1"` and use the
`noodle module-api ...` namespace instead of relying on internal helper
subcommands.

## Stability

- Manifest `api_version` is required for new modules.
- The daemon currently supports `v1`.
- Manifests with unsupported `api_version` values are ignored by discovery.
- First-party modules use this same contract, so the documented path is the
  exercised path.

## Manifest

Minimal manifest shape:

```json
{
  "api_version": "v1",
  "id": "example",
  "handles_events": ["slash_command"],
  "slash_commands": [],
  "uses_tools": [],
  "exports_tools": [],
  "command": ["python3", "${MODULE_DIR}/module.py"]
}
```

Fields:

- `api_version`
  The module host contract version. Today: `"v1"`.
- `id`
  Stable module id used in config and dispatch.
- `handles_events`
  Event names the module can handle.
- `slash_commands`
  Slash-command definitions for explicit slash routing.
- `uses_tools`
  Built-in tool ids this module expects to use through the host.
- `exports_tools`
  MCP tool names this module exports.
- `command`
  Process command to launch. `${MODULE_DIR}` is expanded by the host.

## Request

The host writes one JSON request to the module's stdin:

```json
{
  "api_version": "v1",
  "event": "command_not_found",
  "input": "oo hello",
  "cwd": "/path/to/cwd",
  "shell": "zsh",
  "exit_status": 127,
  "recent_command": "",
  "selected_command": "",
  "debug": false,
  "stream": true,
  "config_path": "/Users/alex/.noodle/config.json",
  "host": {
    "binary_path": "/Users/alex/.noodle/bin/noodle",
    "module_api": {
      "version": "v1",
      "command_prefix": [
        "/Users/alex/.noodle/bin/noodle",
        "module-api"
      ],
      "capabilities": [
        "info",
        "stream_envelopes_v1",
        "model_output",
        "execution_run",
        "execution_resume_permission",
        "memory_context",
        "memory_record_turns",
        "workspace_context",
        "tool_list",
        "tool_call",
        "tool_batch"
      ]
    },
    "module_order": ["utils", "memory", "scripting", "todo", "chat", "typos"],
    "slash_commands": [],
    "tool_counts": {"chat": 18}
  },
  "config": {}
}
```

Important fields:

- `api_version`
  Request schema version. Today: `"v1"`.
- `stream`
  When `true`, the module may stream envelopes instead of returning one final
  JSON object only.
- `config`
  Effective runtime config already resolved by the host.
- `host.module_api.command_prefix`
  Stable command prefix modules should use when calling back into the host.

## Response

Non-streaming modules write exactly one JSON object:

```json
{"ok": true, "payload": {"action": "message", "plugin": "example", "message": "hi"}, "error": null}
```

Streaming modules write NDJSON envelopes:

```json
{"type":"action","payload":{"action":"tool_step","plugin":"chat","tool":"file_read","status":"running","summary":"Reading file"}}
{"type":"action","payload":{"action":"tool_step","plugin":"chat","tool":"file_read","status":"done","summary":"File read"}}
{"type":"final","ok":true,"payload":{"action":"message","plugin":"chat","message":"done"}}
```

Supported streamed envelope types:

- `action`
  A streamed daemon action payload.
- `final`
  The final payload for the request.
- `error`
  A terminal streamed error.

## Host Commands

Modules should call the host through `host.module_api.command_prefix`, which is
normally:

```sh
noodle module-api ...
```

Supported `v1` subcommands:

- `info`
  Return module API version, command prefix, and capabilities as JSON.
- `model-output --config <path>`
  Read a prompt from stdin or `--prompt` and return provider output.
- `execution-run --config <path> [--stream]`
  Run the host execution engine against a JSON request body.
- `execution-resume-permission --config <path> --permission-id <id> --decision <allow|deny> [--stream]`
  Resume a paused execution after a permission prompt.
- `memory-context --config <path> --plugin <id>`
  Return the host-compiled prompt memory block for a plugin.
- `memory-record-turns --config <path> --plugin <id> --user-text <text> --payload <json>`
  Append a user/assistant turn pair and trigger memory compilation policy.
- `workspace-context --cwd <path>`
  Return the host-generated workspace context sections as JSON.
- `tool-list --config <path> [--plugin <id>]`
  Return registered built-in tool definitions for a plugin.
- `tool-call --config <path> --tool <id> --args '<json>'`
  Invoke one built-in tool directly.
- `tool-batch --config <path> --calls '<json array>'`
  Invoke several built-in tools in order.

## Recommendation

For model-assisted modules, prefer:

1. Build request-specific prompt/context in the module.
2. Call `module-api execution-run`.
3. Stream host actions through unchanged.
4. Call `module-api memory-record-turns` after the final payload.

For deterministic modules, read the request JSON directly and return a final
payload without invoking the host.
