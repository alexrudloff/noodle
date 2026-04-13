# noodle

`noodle` is a macOS-first terminal companion for `zsh`.

It combines:

- a thin `zsh` adapter
- a local Rust daemon
- a shared SQLite memory layer
- a daemon-owned tool registry
- a launchd-managed background service

The shell adapter forwards events to the daemon. The daemon decides what to do, uses tools, stores memory, and streams actions back to the shell for rendering.

## Overview

`noodle` ships with five daemon plugins:

- [utils](docs/plugins/utils/README.md) - deterministic slash commands for help, status, reload, and config inspection/editing
- [memory](docs/plugins/memory/README.md) - deterministic inspection, search, and clearing of noodle's shared memory
- [todo](docs/plugins/todo/README.md) - a small terminal todo list stored in shared memory
- [chat](docs/plugins/chat/README.md) - the main agentic assistant with tool use, planning, tasks, and interactive shell support
- [typos](docs/plugins/typos/README.md) - typo recovery for `command not found` and optional command-error fallback

Each plugin has its own README with behavior, commands, and configuration.

## How It Works

The architecture is deliberately split:

- `plugin/noodle.plugin.zsh`
  Captures shell events and renders daemon actions
- `src/main.rs`
  Rust client, local commands, daemon server, provider calls, and memory orchestration
- `src/tooling.rs`
  Built-in tools, plugin manifests, slash-command registry, and permission classes
- `src/executor.rs`
  Tool loop, task execution, replanning, and permission resume
- `src/context_builder.rs`
  Prompt and tool-context assembly
- `src/interactive_shell.rs`
  PTY-backed interactive shell runtime

The `zsh` layer is not the source of truth. The daemon owns:

- plugin dispatch
- tool definitions and invocation
- MCP exposure
- shared memory
- task persistence
- provider calls
- permission decisions and resume state

## Install

```sh
./scripts/install.sh
```

That script:

- builds the Rust binary
- installs files into `~/.noodle`
- installs a launch agent at `~/Library/LaunchAgents/com.noodle.daemon.plist`
- bootstraps and kickstarts the daemon with `launchctl`

Then add this to `~/.zshrc`:

```sh
source "$HOME/.noodle/plugin/noodle.plugin.zsh"
```

Config lives at:

```text
~/.noodle/config.json
```

## Quick Start

Chat:

```sh
oo what changed in this repo?
, summarize the readme
```

Deterministic slash commands:

```sh
/help
/status
/reload
/memory
/todo add document the repo
/todo list
```

Direct CLI utilities:

```sh
~/.noodle/bin/noodle tool-list --config ~/.noodle/config.json --plugin chat
~/.noodle/bin/noodle task-list --config ~/.noodle/config.json
~/.noodle/bin/noodle mcp
```

## Built-In Tools

The daemon exposes built-in primitives, not plugin-owned tools.

Tier 1:

- `memory_query`
- `file_read`
- `path_search`
- `glob`
- `grep`
- `web_fetch`
- `web_search`

Tier 2:

- `file_write`
- `file_edit`
- `shell_exec`
- `interactive_shell_start`
- `interactive_shell_read`
- `interactive_shell_write`
- `interactive_shell_key`
- `interactive_shell_close`

Tier 3:

- `mcp_resource_read`
- `task_note_write`
- `agent_handoff_create`

`web_search` uses `duckduckgo_html` by default and can optionally use Brave Search API.

## Configuration Reference

The canonical example file is [config/config.example.json](config/config.example.json).

### Top-Level Settings

- `provider`
  Model backend. Supported values: `openai_responses`, `openai_compatible`, `anthropic`, `stub`
- `base_url`
  Provider base URL
- `api_key`
  Provider API key
- `model`
  Model name
- `max_tokens`
  Maximum model output tokens
- `reasoning_effort`
  Reasoning level for providers that support it
- `timeout_seconds`
  HTTP timeout for provider requests. Default is `20` if omitted
- `soul`
  High-level assistant identity block included in prompts
- `debug`
  Legacy compatibility key. Prefer `runtime.debug`

### `runtime`

- `runtime.debug`
  Enables debug behavior and extra logging paths
- `runtime.auto_run`
  Controls whether inferred typo fixes auto-run or only display
- `runtime.enable_error_fallback`
  Enables forwarding non-`127` command failures into the typo/error flow
- `runtime.max_retry_depth`
  Prevents recursive command retry loops

### `permissions`

- `permissions.classes.read_only`
- `permissions.classes.network_read`
- `permissions.classes.local_write`
- `permissions.classes.shell_exec`
- `permissions.classes.interactive_shell`
- `permissions.classes.external`

Each class accepts:

- `allow`
- `ask`
- `deny`

Optional per-tool overrides live under:

- `permissions.tools.<tool_name>`

### `search`

- `search.provider`
  Search backend for `web_search`. Supported values: `duckduckgo_html`, `brave_api`
- `search.brave.api_key`
  Brave Search API key
- `search.brave.base_url`
  Brave Search API endpoint. Default: `https://api.search.brave.com/res/v1/web/search`
- `search.brave.country`
  Brave query country code. Default: `us`
- `search.brave.search_lang`
  Brave query language. Default: `en`

### `memory`

- `memory.path`
  SQLite database location

Plugin-specific memory settings are documented in the plugin READMEs:

- [chat memory settings](docs/plugins/chat/README.md#memory-settings)
- [todo memory settings](docs/plugins/todo/README.md#configuration)
- [typos memory settings](docs/plugins/typos/README.md#memory-settings)

### `plugins`

- `plugins.order`
  Ordered list of enabled daemon plugins

Every plugin block may define:

- `plugins.<plugin>.uses_tools`
  Base allowlist of built-in tools for that plugin
- `plugins.<plugin>.tool_availability`
  Per-tool boolean override map layered on top of `uses_tools`
- `plugins.<plugin>.exports_tools`
  MCP-exposed tool names owned by that plugin

Plugin-specific settings are documented here:

- [utils plugin config](docs/plugins/utils/README.md#configuration)
- [memory plugin config](docs/plugins/memory/README.md#configuration)
- [todo plugin config](docs/plugins/todo/README.md#configuration)
- [chat plugin config](docs/plugins/chat/README.md#configuration)
- [typos plugin config](docs/plugins/typos/README.md#configuration)

### `stub`

The `stub` provider is for tests and deterministic harnesses.

- `stub.mode`
- `stub.default_response`
- `stub.matchers`

This is primarily useful for local development and the e2e harness, not normal interactive use.

## Environment Overrides

These environment variables are supported by the current runtime and adapter.

Provider and model:

- `NOODLE_CONFIG`
- `NOODLE_PROVIDER`
- `NOODLE_BASE_URL`
- `NOODLE_API_KEY`
- `NOODLE_MODEL`
- `NOODLE_REASONING_EFFORT`
- `NOODLE_MAX_TOKENS`
- `NOODLE_TIMEOUT_SECONDS`

Prompt and chat behavior:

- `NOODLE_CHAT_PREFIX`
- `NOODLE_CHAT_INCLUDE_TOOL_CONTEXT`
- `NOODLE_CHAT_PROMPT`
- `NOODLE_PROMPT`

Runtime and shell behavior:

- `NOODLE_DEBUG`
- `NOODLE_AUTO_RUN`
- `NOODLE_ENABLE_ERROR_FALLBACK`
- `NOODLE_MAX_RETRY_DEPTH`
- `NOODLE_PLUGIN_ORDER`
- `NOODLE_SELECTION_MODE`

Memory and search:

- `NOODLE_MEMORY_DB`
- `NOODLE_SEARCH_PROVIDER`
- `NOODLE_BRAVE_SEARCH_API_KEY`
- `BRAVE_SEARCH_API_KEY`
- `NOODLE_BRAVE_SEARCH_BASE_URL`

Adapter and daemon wiring:

- `NOODLE_HELPER`
- `NOODLE_SOCKET`
- `NOODLE_PIDFILE`

## Slash Commands

Current built-in slash commands:

- `/help`
- `/status`
- `/reload`
- `/config help`
- `/config show`
- `/config show <key>`
- `/config get <key>`
- `/config set <key> <value>`
- `/config unset <key>`
- `/memory`
- `/memory help`
- `/memory search <term>`
- `/memory clear <plugin|all>`
- `/todo list`
- `/todo help`
- `/todo add <task>`
- `/todo / <id>`
- `/todo x <id>`
- `/todo done <id>`
- `/todo reopen <id>`
- `/todo remove <id>`
- `/todo rm <id>`
- `/todo show <id>`
- `/todo clear-done`

## Tasks And MCP

Planned work is persisted as task records in shared memory.

Useful commands:

```sh
~/.noodle/bin/noodle task-list --config ~/.noodle/config.json
~/.noodle/bin/noodle task-show --config ~/.noodle/config.json --task-id <task-id>
~/.noodle/bin/noodle task-resume --config ~/.noodle/config.json --task-id <task-id>
~/.noodle/bin/noodle task-cancel --config ~/.noodle/config.json --task-id <task-id>
```

`noodle` also exposes an MCP stdio server:

```sh
~/.noodle/bin/noodle mcp
```

Today the main exported MCP tool is `chat.send`, owned by the `chat` plugin.

## Testing

End-to-end coverage:

```sh
./scripts/test-e2e.sh
```

Builtin tool harness:

```sh
./scripts/test-tools.sh
```

The harness uses the built-in stub provider plus local bypass mode so development stays deterministic and fast.

## Repository Layout

- `plugin/noodle.plugin.zsh`
  `zsh` adapter
- `src/main.rs`
  CLI entrypoint, daemon, provider calls, and memory orchestration
- `src/tooling.rs`
  Tool registry, plugin manifests, slash-command registry, permissions
- `src/executor.rs`
  Tool loop, tasks, replanning, interactive progress
- `src/interactive_shell.rs`
  PTY-backed interactive shell runtime
- `src/memory_commands.rs`
  Deterministic `/memory ...` handler
- `src/todo.rs`
  Deterministic `/todo ...` handler
- `src/utils.rs`
  Deterministic `/help`, `/status`, `/reload`, `/config ...` handler
- `config/config.example.json`
  Example configuration
- `docs/plugins/*/README.md`
  Per-plugin docs for GitHub distribution
