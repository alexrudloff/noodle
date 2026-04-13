# CLAUDE.md — noodle

## Purpose

`noodle` is a macOS-first terminal companion for `zsh`. It pairs a thin `zsh`
adapter with a local Rust daemon and a shared SQLite memory layer to provide
an in-terminal agent (`oo ...` / `, ...`), `command_not_found` typo correction,
and a primitive-based tool/MCP surface. The daemon is launched as a user-level
`launchd` service; the client binary and `zsh` plugin talk to it over a Unix
socket.

## Architecture

Three pieces, one real plugin host:

1. **`zsh` adapter** (`plugin/noodle.plugin.zsh`) — captures a small set of
   shell events (`oo`, configurable prefix, `command_not_found`, optional
   `command_error`) and forwards them to the daemon. Renders actions
   (`message`, `run`, `select`, `permission_request`, tool/session
   lifecycle) back in the terminal, including an avatar animation and
   selection prompts.
2. **Rust daemon + client** (`src/`) — single binary `noodle` that:
   - serves a Unix socket (`~/.noodle/noodle.sock`) as `noodle daemon`
   - runs an MCP stdio server as `noodle mcp`
   - dispatches events to built-in plugins in configured order
   - owns model provider calls, memory, tool registry, permissions
   - streams live actions to the adapter during long-running work
3. **Shared SQLite memory** at `~/.noodle/memory.db`, exposing three layers:
   - `events` (raw immutable history)
   - `state` (derived counters / fast lookups)
   - `artifacts` (compiled memory objects, incl. task records)

Six built-in plugins ship today: `utils`, `memory`, `scripting`, `todo`,
`chat`, and `typos`. They are all implemented inside the daemon binary itself
— there is no out-of-process plugin API yet.

## Key modules

All Rust lives in a single binary crate (`Cargo.toml` → `noodle`). Module
responsibilities:

- `src/main.rs` (~2.9k LOC) — argument parsing, daemon socket server,
  MCP stdio server, request routing (`to_request`/`from_request`),
  `execute_local` / `execute_local_stream`, config loading, env overrides,
  memory connection + CRUD helpers, provider calls.
- `src/engine.rs` — thin re-export surface over `executor`, plus config
  helpers (`plugin_tool_calling_enabled`, `plugin_task_execution_enabled`,
  `plugin_max_tool_rounds`, `plugin_max_replans`, `plugin_tools_for_config`).
- `src/executor.rs` (~2.5k LOC) — the tool-calling loop, task execution,
  permission resume, replanning. Entry points:
  `run_chat_execution`, `resume_chat_execution_from_permission`,
  `resume_task_execution`.
- `src/tooling.rs` (~1.8k LOC) — tool registry, `ToolDefinition`,
  `ToolTier` (Tier1/2/3), `ToolPermissionClass`, plugin manifests,
  event matching, builtin tool implementations (`invoke_builtin_tool`,
  `tools_for_plugin`, `exported_mcp_tools`), memory artifact helpers.
- `src/interactive_shell.rs` — PTY-backed interactive shell primitives
  using `portable-pty` for transport and `vt100` for screen-state parsing.
  Exposes `interactive_shell_{start,read,write,close,key}` used by the
  corresponding Tier-2 tools.
- `src/context_builder.rs` — centralized prompt/context assembly
  (`build_chat_base_prompt`, `build_event_prompt`, `EventPromptInput`).
- `src/planner.rs` — planning directives and task-plan parsing
  (`TaskPlan`, `TaskStep`).
- `src/tasks.rs` — durable task records and resumable runtime state
  (`TaskRecord`, `TaskStepRecord`, `TaskRuntimeState`,
  `list_task_records`, `load_task_record`, `load_task_runtime_state`,
  `cancel_task`). Tasks are persisted as memory artifacts.
- `src/actions.rs` — the `DaemonAction` enum the daemon streams back to
  the adapter (`Message`, `Ask`, `Run`, `Select`, `PermissionRequest`,
  `ToolStep`, `Session{Started,Input,Output,Closed}`, task lifecycle).
- `src/permissions.rs` — permission class resolution against
  `permissions.classes` / `permissions.tools` in config.

## Tool tiers

- **Tier 1 (read-ish):** `memory_query`, `file_read`, `path_search`, `glob`,
  `grep`, `web_fetch`, `web_search`.
- **Tier 2 (write / exec):** `file_write`, `file_edit`, `shell_exec`,
  `interactive_shell_{start,read,write,close}`.
- **Tier 3 (orchestration primitives):** `mcp_resource_read`,
  `task_note_write`, `agent_handoff_create`.

Default permission policy: `read_only`/`network_read` → `allow`;
`local_write`/`shell_exec`/`interactive_shell`/`external` → `ask`.
Overridable in `config.json` under `permissions.classes` and
`permissions.tools`.

## Entry points & CLI

Single binary `noodle` with subcommands (see `parse_args` in
`src/main.rs:182`):

- `noodle daemon [--socket ...]` — run the Unix-socket daemon.
- `noodle mcp` — stdio MCP server.
- `noodle runtime-config --config ...` — dump effective config.
- `noodle config-value --config ... --key ... [--fallback ...]` /
  `config-list` — extract config values for the `zsh` adapter.
- `noodle payload-fields [--payload ...]` — parse adapter payloads.
- `noodle tool-list --config ... [--plugin chat]` — list registered tools.
- `noodle tool-call --config ... --tool ... --args '<json>'` — invoke one.
- `noodle tool-batch --config ... --calls '<json array>'` — batch invoke
  (supports cross-call references via `resolve_batch_args`).
- `noodle task-{list,show,resume,cancel} --config ... [--task-id ...]`.
- Default (no subcommand): `--mode ... --input ... --cwd ... [--shell ...]
  [--exit-status ...] [--recent-command ...] [--selected-command ...]
  [--config ...] [--stream]` — the event-forwarding path used by the
  `zsh` adapter.

Env var `NOODLE_BYPASS_DAEMON=1` routes requests through `execute_local`
instead of the socket (used by tests and offline tool calls).

## Scripts

- `scripts/install.sh` — installs client, plugin, and config to `~/.noodle`;
  installs the user launch agent at
  `~/Library/LaunchAgents/com.noodle.daemon.plist`; bootstraps and
  kickstarts the daemon with `launchctl`.
- `scripts/test-e2e.sh` — shells out to the Rust integration tests
  (`cargo test --test e2e`).
- `scripts/test-tools.sh` — narrower harness that forces noodle to exercise
  every builtin tool plus a stub-model chat harness.

## Tests

- `tests/e2e.rs` (~1.9k LOC) — single end-to-end integration binary that
  exercises the sourced `zsh` adapter, the Rust binary, chat via `oo`,
  chat via the configurable prefix, typo correction, and direct tool
  registry / builtin tool calls. Uses a built-in stub provider and
  `NOODLE_BYPASS_DAEMON=1` for determinism.
- Real `launchctl`-managed daemon behavior is only smoke-tested manually.

## Build & run

```sh
# build
cargo build --release

# run full end-to-end tests
./scripts/test-e2e.sh
# or: cargo test --test e2e

# run tool coverage harness
./scripts/test-tools.sh

# local install (macOS)
./scripts/install.sh
source "$HOME/.noodle/plugin/noodle.plugin.zsh"

# direct tool inspection / invocation without the daemon
NOODLE_BYPASS_DAEMON=1 ./target/release/noodle tool-list \
  --config ./config/config.example.json --plugin chat
```

Config lives at `~/.noodle/config.json`; see `config/config.example.json`
for the full shape (provider, `soul`, `runtime`, `permissions`, `memory`,
`plugins.{utils,memory,scripting,todo,chat,typos}` with `uses_tools`, `exports_tools`,
`tool_calling`, `task_execution`, `max_tool_rounds`, `max_replans`).

## Dependencies

Intentionally small (`Cargo.toml`):

- `reqwest` (blocking, rustls-tls) — model provider HTTP.
- `serde` / `serde_json` — config, IPC payloads, tool args, memory blobs.
- `rusqlite` — SQLite memory store.
- `vt100` + `portable-pty` — interactive shell screen parsing and PTY.

Rust edition 2024. Single binary crate, no workspace.

## Conventions & patterns

- **Everything is one binary.** Client, daemon, MCP server, tool runner,
  and task CLI are all dispatched from `main.rs` via the `Command` enum.
- **Config is JSON-pointer addressable** via `lookup`/`value_or_env`; any
  setting can be overridden with a `NOODLE_*` env var (see README
  "Environment Overrides").
- **Tools are daemon primitives**, not plugin-owned behavior. Plugins
  declare `uses_tools` / `exports_tools` / `handles_events` in
  `PluginManifest`; the daemon owns schemas, permission classes,
  invocation, and MCP exposure.
- **Streaming actions:** long-running requests stream `DaemonAction`s
  incrementally via `execute_local_stream` / `handle_streaming_request`
  so tool steps, task progress, and interactive-shell I/O show up live.
- **Tool-calling protocol** is a tiny text format — `TOOL:`, `PLAN:`,
  `STEP:`, `FINAL:` — kept provider-agnostic so OpenAI Responses,
  OpenAI-compatible chat, and Anthropic-compatible APIs can all drive it.
- **Permission resume:** when a tool needs approval, the daemon emits
  `permission_request`, suspends, and later resumes the exact pending
  step via `resume_chat_execution_from_permission`.
- **Task persistence:** `TaskRecord` is stored as a memory artifact;
  `TaskRuntimeState` holds resumable execution state (remaining steps,
  tool turns, replans remaining) so `task-resume` can pick up after a
  pause.
- No `TODO`/`FIXME` markers in `src/` — rough edges live in the README's
  "Not done" list rather than in code comments.

## Rough edges / open work

From README "Not done" and observable state:

- Broad `zsh` event forwarding beyond the current small set.
- Third-party plugin API — today `chat` and `typos` are hard-coded in the
  daemon binary; there is no out-of-process plugin loader.
- Richer daemon lifecycle management (the `launchctl` plist is the entire
  story).
- Memory linting / maintenance passes.
- Non-`zsh` adapters.
- Stronger permission prompts around Tier 2 execution tools.
- Replanning and recovery when a planned step fails (there is a
  `max_replans` knob and `resume_task_execution`, but it's early).
- Richer task progress actions in the adapter.
- A stray empty `dir` file and `noodle-memory.db` in the repo root look
  like dev artifacts, not intentional fixtures.
- `src/main.rs` and `src/executor.rs` are each ~2.5–2.9k LOC — the
  natural next refactor target if the project grows.
