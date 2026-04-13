# chat plugin

The `chat` plugin is noodle's main agentic assistant.

It can:

- answer direct shell questions
- inspect the current workspace
- use built-in tools
- create and execute short plans
- persist task state
- drive interactive terminal sessions through the PTY-backed `interactive_shell_*` tools

## Entry Points

- `oo ...`
- the configured prefix, default `, ...`
- MCP tool export: `chat.send`

## Capabilities

- workspace-aware prompts based on current directory and repo state
- provider-agnostic tool-calling loop
- short multi-step plans with resumable task state
- permission requests with exact-step resume
- interactive CLI driving through `interactive_shell_*`

## Tooling

The `chat` plugin is the main consumer of the daemon tool registry.

Default `uses_tools`:

- `memory_query`
- `file_read`
- `path_search`
- `glob`
- `grep`
- `web_fetch`
- `web_search`
- `file_write`
- `file_edit`
- `shell_exec`
- `interactive_shell_start`
- `interactive_shell_read`
- `interactive_shell_write`
- `interactive_shell_key`
- `interactive_shell_close`
- `mcp_resource_read`
- `task_note_write`
- `agent_handoff_create`

## Configuration

### Plugin block

- `plugins.chat.prefix`
  Shell prefix alias for chat. Default: `,`
- `plugins.chat.include_tool_context`
  Whether extra tool context is included in prompts. Default: `0`
- `plugins.chat.tool_calling`
  Enables the daemon tool loop. Default: `1`
- `plugins.chat.task_execution`
  Enables short plan execution and task persistence. Default: `1`
- `plugins.chat.max_tool_rounds`
  Maximum tool-loop turns. Default: `24`
- `plugins.chat.max_replans`
  Maximum replans after failure. Default: `1`
- `plugins.chat.uses_tools`
  Base builtin-tool allowlist
- `plugins.chat.tool_availability`
  Per-tool boolean overrides layered on top of `uses_tools`
- `plugins.chat.exports_tools`
  MCP exports. Default: `["chat.send"]`
- `plugins.chat.prompt`
  Main chat system prompt

### Memory settings

- `memory.chat.recent_turn_limit`
  How many recent chat turns are retained
- `memory.chat.context_turn_limit`
  How many turns are reintroduced into future prompts
- `memory.chat.summary_max_chars`
  Compiled memory summary size target
- `memory.chat.compile_after_events`
  Number of new events before recompiling chat memory
- `memory.chat.compile_prompt`
  Prompt used to compress durable chat memory

### Related global settings

- `permissions.classes.*`
  Governs approvals for chat tools
- `permissions.tools.<tool_name>`
  Per-tool overrides
- `search.*`
  Governs the backend used by `web_search`
- `provider`, `base_url`, `api_key`, `model`, `max_tokens`, `reasoning_effort`, `timeout_seconds`
  Governs model execution

## Environment Overrides

Relevant overrides for `chat`:

- `NOODLE_CHAT_PREFIX`
- `NOODLE_CHAT_INCLUDE_TOOL_CONTEXT`
- `NOODLE_CHAT_PROMPT`
- `NOODLE_PROVIDER`
- `NOODLE_BASE_URL`
- `NOODLE_API_KEY`
- `NOODLE_MODEL`
- `NOODLE_MAX_TOKENS`
- `NOODLE_REASONING_EFFORT`
- `NOODLE_TIMEOUT_SECONDS`
- `NOODLE_SEARCH_PROVIDER`

## Notes

- `chat` is the least deterministic plugin by design
- tool use, task execution, and interactive shell control all live here
- `web_search` is performed by noodle itself through the builtin tool, not by model memory
