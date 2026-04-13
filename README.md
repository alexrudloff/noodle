<div align="center">

```
███╗   ██╗ ██████╗  ██████╗ ██████╗ ██╗     ███████╗
████╗  ██║██╔═══██╗██╔═══██╗██╔══██╗██║     ██╔════╝
██╔██╗ ██║██║   ██║██║   ██║██║  ██║██║     █████╗
██║╚██╗██║██║   ██║██║   ██║██║  ██║██║     ██╔══╝
██║ ╚████║╚██████╔╝╚██████╔╝██████╔╝███████╗███████╗
╚═╝  ╚═══╝ ╚═════╝  ╚═════╝ ╚═════╝ ╚══════╝╚══════╝
```

### A local-first terminal companion for `zsh`.

**Chat with your repo. Fix typos on the fly. Run an agentic assistant — without ever leaving your shell.**

![platform](https://img.shields.io/badge/platform-macOS-0a0a0a?style=flat-square)
![shell](https://img.shields.io/badge/shell-zsh-4b5563?style=flat-square)
![core](https://img.shields.io/badge/core-Rust-dea584?style=flat-square)
![memory](https://img.shields.io/badge/memory-SQLite-003b57?style=flat-square)
![mcp](https://img.shields.io/badge/protocol-MCP-6b46c1?style=flat-square)

[Install](#install) · [Quick Start](#quick-start) · [Plugins](#plugins) · [Tools](#built-in-tools) · [Config](#configuration) · [MCP](#tasks--mcp)

</div>

---

## What is noodle?

`noodle` is a tiny shell adapter married to a local Rust daemon and a shared SQLite brain. It turns your terminal into a place where you can:

- **Talk to your repo.** `oo what changed in this repo?` — full tool use, planning, file edits, interactive shells.
- **Recover from typos.** `git stauts` becomes `git status`, automatically or with a short menu.
- **Script against shared state.** `/kv`, `/todo`, `/memory` — deterministic primitives that survive across shells.
- **Plug into anything.** Ships as an MCP stdio server, so Claude, Cursor, and other MCP clients can call into it.

Everything runs on your machine. The daemon is a single Rust binary, launched by `launchd`, talking to your shell over a Unix socket.

## Why noodle?

|                         |                                                                                                              |
| ----------------------- | ------------------------------------------------------------------------------------------------------------ |
| **Local first**         | A single Rust binary. No Electron, no cloud state, no background Node process. Your memory stays on disk.   |
| **Shell native**        | A thin `zsh` plugin — not a new terminal, not a wrapper. Works with your prompt, your aliases, your muscle memory. |
| **Agentic when it counts** | Tool use, planning, permission gates, and resumable tasks — but only the `chat` plugin pays for it.       |
| **Deterministic where it matters** | `/help`, `/status`, `/memory`, `/todo`, `/kv` are plain code paths. Fast, predictable, scriptable.   |
| **MCP exposed**         | Every useful primitive surfaces through an MCP stdio server so other agents can drive noodle too.           |
| **Small surface area**  | One binary, one socket, one SQLite file, one launch agent. Easy to reason about, easy to rip out.           |

---

## Quick Start

```sh
# Chat with the agent
oo what changed in this repo?
, summarize the readme

# Deterministic primitives
/help
/status
/memory
/kv set session-token abc123 --ttl 5m
/todo add document the repo
/todo list

# Direct CLI
~/.noodle/bin/noodle tool-list --config ~/.noodle/config.json --plugin chat
~/.noodle/bin/noodle task-list --config ~/.noodle/config.json
~/.noodle/bin/noodle mcp
```

Two ways to invoke the agent:

- **`oo ...`** — the always-on entrypoint.
- **`, ...`** — the configurable chat prefix (`NOODLE_CHAT_PREFIX`).

---

## Install

One-liner:

```sh
./scripts/install.sh
```

The installer:

1. Builds the Rust binary.
2. Installs files into `~/.noodle`.
3. Drops a launch agent at `~/Library/LaunchAgents/com.noodle.daemon.plist`.
4. Bootstraps and kickstarts the daemon via `launchctl`.
5. On first run, prompts for provider / model / API key and writes `~/.noodle/config.json`.

Then add this to your `~/.zshrc`:

```sh
source "$HOME/.noodle/plugin/noodle.plugin.zsh"
```

<details>
<summary><b>Non-interactive install</b></summary>

Skip prompts entirely:

```sh
NOODLE_INSTALL_CONFIGURE_LLM=0 ./scripts/install.sh
```

Preseed values:

```sh
NOODLE_INSTALL_PROVIDER=openai_responses \
NOODLE_INSTALL_MODEL=gpt-5 \
NOODLE_INSTALL_API_KEY=... \
./scripts/install.sh
```

Config lives at `~/.noodle/config.json`.

</details>

---

## How It Works

Three pieces, one real host:

```
  ┌─────────────────┐        ┌──────────────────────┐        ┌──────────────┐
  │  zsh adapter    │  unix  │   noodle daemon      │        │   SQLite     │
  │  noodle.plugin  │◀──────▶│   (Rust, launchd)    │◀──────▶│  memory.db   │
  │    .zsh         │ socket │   tools · plugins    │        │  events ·    │
  └─────────────────┘        │   memory · permissions│       │  state ·     │
           ▲                 └──────────┬───────────┘        │  artifacts   │
           │                            │                    └──────────────┘
      renders                           │ provider calls
      actions                           ▼
                                 ┌──────────────┐
                                 │ OpenAI /     │
                                 │ Anthropic /  │
                                 │ OpenAI-compat│
                                 └──────────────┘
```

- **`plugin/noodle.plugin.zsh`** — captures shell events (`oo`, chat prefix, `command_not_found`, optional `command_error`) and renders streamed daemon actions: messages, runs, selections, permission requests, tool steps, session lifecycle, avatar animation.
- **Rust daemon + client** — a single `noodle` binary that serves a Unix socket (`~/.noodle/noodle.sock`), runs the MCP stdio server, owns the tool registry, model provider calls, memory, and permission decisions.
- **Shared SQLite memory** (`~/.noodle/memory.db`) — three layers: immutable `events`, derived `state`, and compiled `artifacts` (including task records).

The `zsh` layer is not the source of truth. The daemon owns plugin dispatch, tool definitions, MCP exposure, shared memory, task persistence, provider calls, and permission decisions.

---

## Plugins

Six daemon plugins ship in the box. They split by job: some are deterministic and fast, some are model-assisted, some exist to make shell workflows less brittle.

| Plugin      | What it is                                                                          | Try it                                           |
| ----------- | ----------------------------------------------------------------------------------- | ------------------------------------------------ |
| [`utils`](docs/plugins/utils/README.md)       | Control plane for noodle. `/help`, `/status`, `/reload`, `/config ...`.         | `/config get plugins.order`                      |
| [`memory`](docs/plugins/memory/README.md)     | Operator view over shared SQLite memory. Summarize, search, clear by plugin.   | `/memory search deploy`                          |
| [`scripting`](docs/plugins/scripting/README.md) | Small shell-scripting primitives. Shared KV with TTL.                        | `/kv set session-token abc123 --ttl 5m`          |
| [`todo`](docs/plugins/todo/README.md)         | Terminal todo list with stable ids, stored in shared memory.                    | `/todo add document plugins`                     |
| [`chat`](docs/plugins/chat/README.md)         | The agent behind `oo` and the chat prefix. Tool use, planning, tasks, shells.   | `oo what changed in this repo?`                  |
| [`typos`](docs/plugins/typos/README.md)       | Typo recovery for `command not found` and optional command-error fallback.      | `git stauts` → `git status`                      |

Each plugin has its own README with behavior, commands, and config.

---

## Built-in Tools

Tools are **daemon primitives**, not plugin-owned behavior. Plugins opt in via `uses_tools` / `exports_tools`; the daemon owns schemas, permissions, invocation, and MCP exposure.

| Tier                    | Tools                                                                                                                                      | Default policy        |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | --------------------- |
| **Tier 1 — read**       | `memory_query` · `file_read` · `path_search` · `glob` · `grep` · `web_fetch` · `web_search`                                                | `allow`               |
| **Tier 2 — write / exec** | `file_write` · `file_edit` · `shell_exec` · `interactive_shell_{start,read,write,key,close}`                                             | `ask`                 |
| **Tier 3 — orchestration** | `mcp_resource_read` · `task_note_write` · `agent_handoff_create`                                                                        | `ask`                 |

Permissions are overridable per-class or per-tool under `permissions.classes` / `permissions.tools` in config.

`web_search` defaults to `duckduckgo_html` and can switch to Brave Search API.

---

## Slash Commands

<details>
<summary><b>All built-in slash commands</b></summary>

**Core**
- `/help`
- `/status`
- `/reload`

**Config**
- `/config help`
- `/config show [key]`
- `/config get <key>`
- `/config set <key> <value>`
- `/config unset <key>`

**Memory**
- `/memory`
- `/memory help`
- `/memory search <term>`
- `/memory clear <plugin|all>`

**Todo**
- `/todo list`
- `/todo help`
- `/todo add <task>`
- `/todo / <id>` · `/todo x <id>` · `/todo done <id>`
- `/todo reopen <id>`
- `/todo remove <id>` · `/todo rm <id>`
- `/todo show <id>`
- `/todo clear-done`

**Scripting**
- `/kv set <key> <value> [--ttl <dur>]`
- `/kv get <key>`
- `/kv unset <key>`

</details>

---

## Tasks & MCP

Planned work is persisted as **task records** in shared memory. Tasks are resumable: if a step requires permission, the daemon suspends, streams a `permission_request`, and later resumes the exact pending step.

```sh
~/.noodle/bin/noodle task-list   --config ~/.noodle/config.json
~/.noodle/bin/noodle task-show   --config ~/.noodle/config.json --task-id <id>
~/.noodle/bin/noodle task-resume --config ~/.noodle/config.json --task-id <id>
~/.noodle/bin/noodle task-cancel --config ~/.noodle/config.json --task-id <id>
```

noodle also exposes an **MCP stdio server**:

```sh
~/.noodle/bin/noodle mcp
```

Today the main exported MCP tool is `chat.send`, owned by the `chat` plugin — so any MCP client can drive the full agent.

---

## Configuration

The canonical example lives at [`config/config.example.json`](config/config.example.json). Every setting can be overridden with a `NOODLE_*` env var.

<details>
<summary><b>Top-level settings</b></summary>

| Key                    | Meaning                                                                     |
| ---------------------- | --------------------------------------------------------------------------- |
| `provider`             | `openai_responses` · `openai_compatible` · `anthropic` · `stub`             |
| `base_url`             | Provider base URL                                                           |
| `api_key`              | Provider API key                                                            |
| `model`                | Model name                                                                  |
| `max_tokens`           | Max model output tokens                                                     |
| `reasoning_effort`     | Reasoning level for providers that support it                               |
| `timeout_seconds`      | HTTP timeout (default `20`)                                                 |
| `soul`                 | High-level assistant identity block included in prompts                     |
| `debug`                | Legacy; prefer `runtime.debug`                                              |

</details>

<details>
<summary><b><code>runtime</code></b></summary>

- `runtime.debug` — extra logging and debug paths
- `runtime.auto_run` — auto-run inferred typo fixes vs. display only
- `runtime.enable_error_fallback` — forward non-`127` failures into the typo/error flow
- `runtime.max_retry_depth` — cap recursive retries

</details>

<details>
<summary><b><code>permissions</code></b></summary>

Classes:

- `permissions.classes.read_only`
- `permissions.classes.network_read`
- `permissions.classes.local_write`
- `permissions.classes.shell_exec`
- `permissions.classes.interactive_shell`
- `permissions.classes.external`

Each accepts `allow`, `ask`, or `deny`. Per-tool overrides go under `permissions.tools.<tool_name>`.

</details>

<details>
<summary><b><code>search</code></b></summary>

- `search.provider` — `duckduckgo_html` or `brave_api`
- `search.brave.api_key`
- `search.brave.base_url` — default `https://api.search.brave.com/res/v1/web/search`
- `search.brave.country` — default `us`
- `search.brave.search_lang` — default `en`

</details>

<details>
<summary><b><code>memory</code></b></summary>

- `memory.path` — SQLite database location

Plugin-specific memory settings live in the plugin READMEs:

- [chat memory settings](docs/plugins/chat/README.md#memory-settings)
- [todo memory settings](docs/plugins/todo/README.md#configuration)
- [typos memory settings](docs/plugins/typos/README.md#memory-settings)

</details>

<details>
<summary><b><code>plugins</code></b></summary>

- `plugins.order` — ordered list of enabled daemon plugins

Every plugin block may define:

- `plugins.<plugin>.uses_tools` — base allowlist of built-in tools
- `plugins.<plugin>.tool_availability` — per-tool boolean override map
- `plugins.<plugin>.exports_tools` — MCP-exposed tool names

Per-plugin config:

- [utils](docs/plugins/utils/README.md#configuration)
- [memory](docs/plugins/memory/README.md#configuration)
- [scripting](docs/plugins/scripting/README.md#configuration)
- [todo](docs/plugins/todo/README.md#configuration)
- [chat](docs/plugins/chat/README.md#configuration)
- [typos](docs/plugins/typos/README.md#configuration)

</details>

<details>
<summary><b><code>stub</code> provider (tests)</b></summary>

- `stub.mode`
- `stub.default_response`
- `stub.matchers`

Used by the e2e harness and local dev, not normal interactive use.

</details>

### Environment overrides

<details>
<summary><b>Provider & model</b></summary>

`NOODLE_CONFIG` · `NOODLE_PROVIDER` · `NOODLE_BASE_URL` · `NOODLE_API_KEY` · `NOODLE_MODEL` · `NOODLE_REASONING_EFFORT` · `NOODLE_MAX_TOKENS` · `NOODLE_TIMEOUT_SECONDS`

</details>

<details>
<summary><b>Prompt & chat behavior</b></summary>

`NOODLE_CHAT_PREFIX` · `NOODLE_CHAT_INCLUDE_TOOL_CONTEXT` · `NOODLE_CHAT_PROMPT` · `NOODLE_PROMPT`

</details>

<details>
<summary><b>Runtime & shell behavior</b></summary>

`NOODLE_DEBUG` · `NOODLE_AUTO_RUN` · `NOODLE_ENABLE_ERROR_FALLBACK` · `NOODLE_MAX_RETRY_DEPTH` · `NOODLE_PLUGIN_ORDER` · `NOODLE_SELECTION_MODE`

</details>

<details>
<summary><b>Memory & search</b></summary>

`NOODLE_MEMORY_DB` · `NOODLE_SEARCH_PROVIDER` · `NOODLE_BRAVE_SEARCH_API_KEY` · `BRAVE_SEARCH_API_KEY` · `NOODLE_BRAVE_SEARCH_BASE_URL`

</details>

<details>
<summary><b>Adapter & daemon wiring</b></summary>

`NOODLE_HELPER` · `NOODLE_SOCKET` · `NOODLE_PIDFILE`

</details>

<details>
<summary><b>Installer only</b></summary>

`NOODLE_INSTALL_CONFIGURE_LLM` · `NOODLE_INSTALL_PROVIDER` · `NOODLE_INSTALL_BASE_URL` · `NOODLE_INSTALL_API_KEY` · `NOODLE_INSTALL_MODEL` · `NOODLE_INSTALL_REASONING_EFFORT` · `NOODLE_INSTALL_TIMEOUT_SECONDS`

</details>

---

## Testing

End-to-end coverage runs against a real sourced `zsh` adapter, the Rust binary, and a stub provider — no network, deterministic output:

```sh
./scripts/test-e2e.sh        # cargo test --test e2e
./scripts/test-tools.sh      # builtin tool coverage harness
```

`NOODLE_BYPASS_DAEMON=1` routes requests through the local executor instead of the socket, used by tests and offline tool calls.

---

## Repository Layout

```
plugin/noodle.plugin.zsh       zsh adapter — events in, actions out
src/main.rs                    CLI, daemon server, provider calls, memory
src/tooling.rs                 tool registry, manifests, permissions
src/executor.rs                tool loop, tasks, replanning, permission resume
src/interactive_shell.rs       PTY-backed interactive shell runtime
src/context_builder.rs         prompt and tool-context assembly
src/planner.rs                 planning directives and task-plan parsing
src/tasks.rs                   durable task records and resumable runtime state
src/actions.rs                 DaemonAction enum streamed back to the adapter
src/permissions.rs             permission class resolution
src/memory_commands.rs         deterministic /memory handler
src/todo.rs                    deterministic /todo handler
src/utils.rs                   deterministic /help /status /reload /config
config/config.example.json     example configuration
docs/plugins/*/README.md       per-plugin docs
```

---

## License

`noodle` is licensed under the Apache License, Version 2.0.
See [LICENSE](LICENSE) for the full text.

---

<div align="center">

Built in Rust. Powered by `zsh`, SQLite, and a launch agent.

</div>
