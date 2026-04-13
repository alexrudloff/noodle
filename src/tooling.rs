use crate::interactive_shell::{
    interactive_shell_close, interactive_shell_key, interactive_shell_read,
    interactive_shell_start, interactive_shell_write,
};
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolTier {
    Tier1,
    Tier2,
    Tier3,
}

impl ToolTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tier1 => "tier1",
            Self::Tier2 => "tier2",
            Self::Tier3 => "tier3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolPermissionClass {
    ReadOnly,
    NetworkRead,
    LocalWrite,
    ShellExec,
    InteractiveShell,
    External,
}

impl ToolPermissionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::NetworkRead => "network_read",
            Self::LocalWrite => "local_write",
            Self::ShellExec => "shell_exec",
            Self::InteractiveShell => "interactive_shell",
            Self::External => "external",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPermissionDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub tier: ToolTier,
    pub permission: ToolPermissionClass,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct PluginManifest {
    pub id: &'static str,
    pub handles_events: Vec<&'static str>,
    pub slash_commands: Vec<&'static str>,
    pub uses_tools: Vec<&'static str>,
    pub exports_mcp_tools: Vec<&'static str>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SlashCommandDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub usage: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallResult {
    pub tool: String,
    pub ok: bool,
    pub output: Value,
}

fn tool_definition(
    name: &'static str,
    description: &'static str,
    tier: ToolTier,
    permission: ToolPermissionClass,
    input_schema: Value,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        tier,
        permission,
        input_schema,
    }
}

pub fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        tool_definition(
            "memory_query",
            "Query noodle memory events, state, and compiled artifacts.",
            ToolTier::Tier1,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "plugin": { "type": "string" },
                    "kind": { "type": "string" },
                    "key_prefix": { "type": "string" },
                    "source": { "type": "string", "enum": ["events", "state", "artifacts", "all"] },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "file_read",
            "Read a local file.",
            ToolTier::Tier1,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "path_search",
            "Find local files or directories by name or path fragment.",
            ToolTier::Tier1,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "root": { "type": "string" },
                    "query": { "type": "string" },
                    "kind": { "type": "string", "enum": ["any", "file", "dir"] },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "glob",
            "Find files recursively by pattern.",
            ToolTier::Tier1,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "root": { "type": "string" },
                    "pattern": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "grep",
            "Search file contents recursively for a plain text pattern.",
            ToolTier::Tier1,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "root": { "type": "string" },
                    "pattern": { "type": "string" },
                    "path_glob": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "web_fetch",
            "Fetch the contents of a URL and return trimmed text.",
            ToolTier::Tier1,
            ToolPermissionClass::NetworkRead,
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "max_chars": { "type": "integer", "minimum": 200 }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "web_search",
            "Search the web and return a small set of result titles and links.",
            ToolTier::Tier1,
            ToolPermissionClass::NetworkRead,
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "file_write",
            "Write a file, replacing its full contents.",
            ToolTier::Tier2,
            ToolPermissionClass::LocalWrite,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "file_edit",
            "Replace text inside a file.",
            ToolTier::Tier2,
            ToolPermissionClass::LocalWrite,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "find": { "type": "string" },
                    "replace": { "type": "string" },
                    "replace_all": { "type": "boolean" }
                },
                "required": ["path", "find", "replace"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "shell_exec",
            "Run a shell command.",
            ToolTier::Tier2,
            ToolPermissionClass::ShellExec,
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string" }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "interactive_shell_start",
            "Start a PTY-backed interactive shell session for a command.",
            ToolTier::Tier2,
            ToolPermissionClass::InteractiveShell,
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string" }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "interactive_shell_read",
            "Read incremental output from an interactive shell session.",
            ToolTier::Tier2,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "since_seq": { "type": "integer", "minimum": 0 },
                    "wait_ms": { "type": "integer", "minimum": 0 },
                    "settle_ms": { "type": "integer", "minimum": 0 },
                    "max_chars": { "type": "integer", "minimum": 256 }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "interactive_shell_write",
            "Write input into an interactive shell session.",
            ToolTier::Tier2,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "text": { "type": "string" },
                    "submit": { "type": "boolean" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "interactive_shell_key",
            "Press a named key in an interactive shell session.",
            ToolTier::Tier2,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "key": { "type": "string" },
                    "repeat": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["session_id", "key"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "interactive_shell_close",
            "Close an interactive shell session.",
            ToolTier::Tier2,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "mcp_resource_read",
            "Read an MCP resource from a configured server. Reserved primitive.",
            ToolTier::Tier3,
            ToolPermissionClass::External,
            json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string" },
                    "uri": { "type": "string" }
                },
                "required": ["server", "uri"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "task_note_write",
            "Write a durable task note into noodle memory as a compiled artifact.",
            ToolTier::Tier3,
            ToolPermissionClass::LocalWrite,
            json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["kind", "content"],
                "additionalProperties": false
            }),
        ),
        tool_definition(
            "agent_handoff_create",
            "Create a durable agent handoff note in noodle memory.",
            ToolTier::Tier3,
            ToolPermissionClass::LocalWrite,
            json!({
                "type": "object",
                "properties": {
                    "agent": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["agent", "content"],
                "additionalProperties": false
            }),
        ),
    ]
}

pub fn plugin_manifest(plugin_id: &str) -> PluginManifest {
    match plugin_id {
        "chat" => PluginManifest {
            id: "chat",
            handles_events: vec!["command_not_found", "command_error"],
            slash_commands: vec![],
            uses_tools: vec![
                "memory_query",
                "file_read",
                "path_search",
                "glob",
                "grep",
                "web_fetch",
                "web_search",
                "file_write",
                "file_edit",
                "shell_exec",
                "interactive_shell_start",
                "interactive_shell_read",
                "interactive_shell_write",
                "interactive_shell_key",
                "interactive_shell_close",
                "mcp_resource_read",
                "task_note_write",
                "agent_handoff_create",
            ],
            exports_mcp_tools: vec!["chat.send"],
        },
        "typos" => PluginManifest {
            id: "typos",
            handles_events: vec!["command_not_found", "command_error"],
            slash_commands: vec![],
            uses_tools: vec![],
            exports_mcp_tools: vec![],
        },
        "utils" => PluginManifest {
            id: "utils",
            handles_events: vec!["slash_command"],
            slash_commands: vec!["help", "status", "reload", "config"],
            uses_tools: vec![],
            exports_mcp_tools: vec![],
        },
        "memory" => PluginManifest {
            id: "memory",
            handles_events: vec!["slash_command"],
            slash_commands: vec!["memory"],
            uses_tools: vec![],
            exports_mcp_tools: vec![],
        },
        "todo" => PluginManifest {
            id: "todo",
            handles_events: vec!["slash_command"],
            slash_commands: vec!["todo"],
            uses_tools: vec![],
            exports_mcp_tools: vec![],
        },
        _ => PluginManifest {
            id: "",
            handles_events: vec![],
            slash_commands: vec![],
            uses_tools: vec![],
            exports_mcp_tools: vec![],
        },
    }
}

pub fn plugin_order(config: &Value) -> Vec<String> {
    if let Some(items) = lookup(config, "plugins.order").and_then(Value::as_array) {
        let plugins = items
            .iter()
            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<Vec<_>>();
        if !plugins.is_empty() {
            return plugins;
        }
    }
    vec![
        "utils".into(),
        "memory".into(),
        "todo".into(),
        "chat".into(),
        "typos".into(),
    ]
}

pub fn enabled_plugin_manifests(config: &Value) -> Vec<PluginManifest> {
    plugin_order(config)
        .into_iter()
        .map(|plugin_id| plugin_manifest(&plugin_id))
        .filter(|manifest| !manifest.id.is_empty())
        .collect()
}

pub fn plugin_matches_request(
    config: &Value,
    manifest: &PluginManifest,
    event: &str,
    input: &str,
) -> bool {
    if !manifest.handles_events.iter().any(|item| *item == event) {
        return false;
    }
    match manifest.id {
        "chat" => chat_matches_request(config, input),
        "typos" => true,
        _ if !manifest.slash_commands.is_empty() => slash_command_matches_request(manifest, input),
        _ => false,
    }
}

pub fn slash_command_definition(name: &str) -> Option<SlashCommandDefinition> {
    match name {
        "todo" => Some(SlashCommandDefinition {
            name: "todo",
            description: "Manage a small terminal todo list stored in noodle memory.",
            usage: "/todo [list|add|/|x|done|reopen|remove|show|clear-done|help]",
        }),
        "help" => Some(SlashCommandDefinition {
            name: "help",
            description: "Show available slash commands and usage.",
            usage: "/help",
        }),
        "status" => Some(SlashCommandDefinition {
            name: "status",
            description: "Show noodle status, active config, plugins, and permissions.",
            usage: "/status",
        }),
        "reload" => Some(SlashCommandDefinition {
            name: "reload",
            description: "Reload noodle runtime config in the current shell.",
            usage: "/reload",
        }),
        "config" => Some(SlashCommandDefinition {
            name: "config",
            description: "Inspect or update noodle config from the terminal.",
            usage: "/config [help|show|get|set|unset] ...",
        }),
        "memory" => Some(SlashCommandDefinition {
            name: "memory",
            description: "Inspect, search, or clear noodle memory state.",
            usage: "/memory [help|search|clear] ...",
        }),
        _ => None,
    }
}

pub fn registered_slash_command_names(config: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for manifest in enabled_plugin_manifests(config) {
        for command in manifest.slash_commands {
            if slash_command_definition(command).is_some() && !names.iter().any(|item| item == command)
            {
                names.push(command.to_string());
            }
        }
    }
    names
}

pub fn slash_command_name(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?;
    let name = rest.split_whitespace().next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub fn slash_command_matches_request(manifest: &PluginManifest, input: &str) -> bool {
    let Some(name) = slash_command_name(input) else {
        return false;
    };
    manifest.slash_commands.iter().any(|command| *command == name)
}

pub fn tools_for_plugin(config: &Value, plugin_id: &str) -> Vec<ToolDefinition> {
    let default_manifest = plugin_manifest(plugin_id);
    let override_key = format!("plugins.{plugin_id}.uses_tools");
    let tool_names = lookup(config, &override_key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or(default_manifest.uses_tools.clone());
    let mut enabled = tool_names
        .iter()
        .map(|name| (*name).to_string())
        .collect::<HashSet<_>>();
    for (tool_name, available) in plugin_tool_availability(config, plugin_id) {
        if available {
            enabled.insert(tool_name);
        } else {
            enabled.remove(&tool_name);
        }
    }
    builtin_tool_definitions()
        .into_iter()
        .filter(|tool| enabled.contains(tool.name))
        .collect()
}

fn plugin_tool_availability(config: &Value, plugin_id: &str) -> HashMap<String, bool> {
    let override_key = format!("plugins.{plugin_id}.tool_availability");
    lookup(config, &override_key)
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(tool, value)| value.as_bool().map(|available| (tool.clone(), available)))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default()
}

pub fn exported_mcp_tool_names(config: &Value, plugin_order: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for plugin_id in plugin_order {
        let manifest = plugin_manifest(plugin_id);
        let override_key = format!("plugins.{plugin_id}.exports_tools");
        let exports = lookup(config, &override_key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                manifest
                    .exports_mcp_tools
                    .iter()
                    .map(|item| item.to_string())
                    .collect()
            });
        for name in exports {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

pub fn mcp_tool_definition(name: &str) -> Option<ToolDefinition> {
    match name {
        "chat.send" => Some(tool_definition(
            "chat.send",
            "Send a plain-text message to noodle's chat plugin and get the reply.",
            ToolTier::Tier1,
            ToolPermissionClass::ReadOnly,
            json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "The message to send to noodle chat."
                    }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
        )),
        _ => None,
    }
}

pub fn exported_mcp_tools(config: &Value) -> Vec<ToolDefinition> {
    let plugin_ids = plugin_order(config);
    exported_mcp_tool_names(config, &plugin_ids)
        .into_iter()
        .filter_map(|name| mcp_tool_definition(&name))
        .collect()
}

pub fn chat_matches_request(config: &Value, input: &str) -> bool {
    let prefix = value_or_env(config, "NOODLE_CHAT_PREFIX", "plugins.chat.prefix", ",");
    input == "oo" || input.starts_with("oo ") || input.starts_with(&prefix)
}

pub fn invoke_builtin_tool(
    config: &Value,
    working_directory: Option<&str>,
    tool_name: &str,
    args: &Value,
) -> Result<ToolCallResult, String> {
    let normalized_args = prepare_builtin_tool_args(tool_name, args, working_directory);
    let output = match tool_name {
        "memory_query" => tool_memory_query(config, &normalized_args)?,
        "file_read" => tool_file_read(&normalized_args)?,
        "path_search" => tool_path_search(&normalized_args)?,
        "glob" => tool_glob(&normalized_args)?,
        "grep" => tool_grep(&normalized_args)?,
        "web_fetch" => tool_web_fetch(&normalized_args)?,
        "web_search" => tool_web_search(config, &normalized_args)?,
        "file_write" => tool_file_write(&normalized_args)?,
        "file_edit" => tool_file_edit(&normalized_args)?,
        "shell_exec" => tool_shell_exec(&normalized_args)?,
        "interactive_shell_start" => interactive_shell_start(&normalized_args)?,
        "interactive_shell_read" => interactive_shell_read(&normalized_args)?,
        "interactive_shell_write" => interactive_shell_write(&normalized_args)?,
        "interactive_shell_key" => interactive_shell_key(&normalized_args)?,
        "interactive_shell_close" => interactive_shell_close(&normalized_args)?,
        "mcp_resource_read" => tool_mcp_resource_read(&normalized_args)?,
        "task_note_write" => tool_task_note_write(config, &normalized_args)?,
        "agent_handoff_create" => tool_agent_handoff_create(config, &normalized_args)?,
        other => return Err(format!("unknown builtin tool: {other}")),
    };
    Ok(ToolCallResult {
        tool: tool_name.to_string(),
        ok: true,
        output,
    })
}

pub fn prepare_builtin_tool_args(
    tool_name: &str,
    args: &Value,
    working_directory: Option<&str>,
) -> Value {
    let Some(cwd) = working_directory.filter(|value| !value.trim().is_empty()) else {
        return args.clone();
    };
    let Some(object) = args.as_object() else {
        return args.clone();
    };
    let mut normalized = object.clone();
    match tool_name {
        "file_read" | "file_write" | "file_edit" => {
            if let Some(path) = normalized.get("path").and_then(Value::as_str) {
                normalized.insert("path".into(), Value::String(resolve_path(cwd, path)));
            }
        }
        "path_search" | "glob" | "grep" => {
            if let Some(root) = normalized.get("root").and_then(Value::as_str) {
                normalized.insert("root".into(), Value::String(resolve_path(cwd, root)));
            } else {
                normalized.insert("root".into(), Value::String(resolve_path(cwd, ".")));
            }
        }
        "shell_exec" | "interactive_shell_start" => {
            if normalized.get("cwd").and_then(Value::as_str).is_none() {
                normalized.insert("cwd".into(), Value::String(resolve_path(cwd, ".")));
            } else if let Some(path) = normalized.get("cwd").and_then(Value::as_str) {
                normalized.insert("cwd".into(), Value::String(resolve_path(cwd, path)));
            }
        }
        _ => {}
    }
    Value::Object(normalized)
}

fn resolve_path(base: &str, path: &str) -> String {
    if path.is_empty() {
        return path.to_string();
    }
    if path.starts_with("~/") || Path::new(path).is_absolute() {
        return path.to_string();
    }
    expand_home(base).join(path).to_string_lossy().to_string()
}

fn tool_memory_query(config: &Value, args: &Value) -> Result<Value, String> {
    let plugin = args.get("plugin").and_then(Value::as_str).unwrap_or("");
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("");
    let key_prefix = args.get("key_prefix").and_then(Value::as_str).unwrap_or("");
    let source = args.get("source").and_then(Value::as_str).unwrap_or("all");
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .clamp(1, 100);
    let conn = memory_connection(config)?;
    let mut result = serde_json::Map::new();
    if source == "events" || source == "all" {
        let mut stmt = conn
            .prepare(
                "SELECT plugin, kind, key, value_json, created_at
                 FROM events
                 WHERE (?1 = '' OR plugin = ?1)
                   AND (?2 = '' OR kind = ?2)
                   AND (?3 = '' OR key LIKE ?3 || '%')
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?4",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map(params![plugin, kind, key_prefix, limit], |row| {
                Ok(json!({
                    "plugin": row.get::<_, String>(0)?,
                    "kind": row.get::<_, String>(1)?,
                    "key": row.get::<_, String>(2)?,
                    "value": serde_json::from_str::<Value>(&row.get::<_, String>(3)?).unwrap_or(Value::Null),
                    "created_at": row.get::<_, i64>(4)?,
                }))
            })
            .map_err(|err| err.to_string())?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|err| err.to_string())?);
        }
        result.insert("events".into(), Value::Array(items));
    }
    if source == "state" || source == "all" {
        let mut stmt = conn
            .prepare(
                "SELECT plugin, key, value_json, updated_at
                 FROM state
                 WHERE (?1 = '' OR plugin = ?1)
                   AND (?3 = '' OR key LIKE ?3 || '%')
                 ORDER BY updated_at DESC
                 LIMIT ?4",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map(params![plugin, kind, key_prefix, limit], |row| {
                Ok(json!({
                    "plugin": row.get::<_, String>(0)?,
                    "key": row.get::<_, String>(1)?,
                    "value": serde_json::from_str::<Value>(&row.get::<_, String>(2)?).unwrap_or(Value::Null),
                    "updated_at": row.get::<_, i64>(3)?,
                }))
            })
            .map_err(|err| err.to_string())?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|err| err.to_string())?);
        }
        result.insert("state".into(), Value::Array(items));
    }
    if source == "artifacts" || source == "all" {
        let mut stmt = conn
            .prepare(
                "SELECT plugin, kind, content, source_json, active, updated_at
                 FROM artifacts
                 WHERE (?1 = '' OR plugin = ?1)
                   AND (?2 = '' OR kind = ?2)
                 ORDER BY updated_at DESC, id DESC
                 LIMIT ?3",
            )
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map(params![plugin, kind, limit], |row| {
                Ok(json!({
                    "plugin": row.get::<_, String>(0)?,
                    "kind": row.get::<_, String>(1)?,
                    "content": row.get::<_, String>(2)?,
                    "source": serde_json::from_str::<Value>(&row.get::<_, String>(3)?).unwrap_or(Value::Null),
                    "active": row.get::<_, i64>(4)? == 1,
                    "updated_at": row.get::<_, i64>(5)?,
                }))
            })
            .map_err(|err| err.to_string())?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|err| err.to_string())?);
        }
        result.insert("artifacts".into(), Value::Array(items));
    }
    Ok(Value::Object(result))
}

fn tool_file_read(args: &Value) -> Result<Value, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "file_read requires path".to_string())?;
    let expanded = expand_home(path);
    let raw = fs::read_to_string(&expanded).map_err(|err| err.to_string())?;
    let content = normalize_file_content(&raw);
    let canonical = fs::canonicalize(&expanded).unwrap_or(expanded.clone());
    let mtime = fs::metadata(&expanded)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);
    Ok(json!({
        "path": canonical.to_string_lossy().to_string(),
        "content": content,
        "mtime": mtime
    }))
}

fn tool_path_search(args: &Value) -> Result<Value, String> {
    let root = args.get("root").and_then(Value::as_str).unwrap_or(".");
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| "path_search requires query".to_string())?;
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("any");
    if !matches!(kind, "any" | "file" | "dir") {
        return Err("path_search kind must be one of: any, file, dir".into());
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(25)
        .min(500) as usize;
    let root_path = expand_home(root);
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for path in spotlight_name_search(&root_path, query, kind, limit) {
        if seen.insert(path.clone()) {
            results.push(path);
            if results.len() >= limit {
                break;
            }
        }
    }

    if results.len() < limit {
        visit_nodes(&root_path, &mut |path| {
            if results.len() >= limit {
                return;
            }
            if !path_kind_matches(path, kind) || !path_matches_query(path, query) {
                return;
            }
            let display = path.to_string_lossy().to_string();
            if seen.insert(display.clone()) {
                results.push(display);
            }
        })?;
    }

    Ok(json!({
        "root": root,
        "query": query,
        "kind": kind,
        "matches": results
    }))
}

fn tool_glob(args: &Value) -> Result<Value, String> {
    let root = args.get("root").and_then(Value::as_str).unwrap_or(".");
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| "glob requires pattern".to_string())?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .min(500) as usize;
    let mut results = Vec::new();
    let root_path = expand_home(root);
    visit_paths(&root_path, &mut |path| {
        if results.len() >= limit {
            return;
        }
        let display = path.to_string_lossy().to_string();
        let relative = path
            .strip_prefix(&root_path)
            .ok()
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if wildcard_match(pattern, &display)
            || (!relative.is_empty() && wildcard_match(pattern, &relative))
            || wildcard_match(pattern, path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
        {
            results.push(display);
        }
    })?;
    Ok(json!({"root": root, "pattern": pattern, "matches": results}))
}

fn tool_grep(args: &Value) -> Result<Value, String> {
    let root = args.get("root").and_then(Value::as_str).unwrap_or(".");
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| "grep requires pattern".to_string())?;
    let path_glob = args.get("path_glob").and_then(Value::as_str).unwrap_or("");
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .min(500) as usize;
    let mut matches = Vec::new();
    let root_path = expand_home(root);
    visit_paths(&root_path, &mut |path| {
        if matches.len() >= limit {
            return;
        }
        let display = path.to_string_lossy().to_string();
        let relative = path
            .strip_prefix(&root_path)
            .ok()
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if !path_glob.is_empty()
            && !(wildcard_match(path_glob, &display)
                || (!relative.is_empty() && wildcard_match(path_glob, &relative))
                || wildcard_match(
                    path_glob,
                    path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                ))
        {
            return;
        }
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => return,
        };
        for (index, line) in content.lines().enumerate() {
            if line.contains(pattern) {
                matches.push(json!({
                    "path": display,
                    "line": index + 1,
                    "text": line,
                }));
                if matches.len() >= limit {
                    break;
                }
            }
        }
    })?;
    Ok(json!({"root": root, "pattern": pattern, "matches": matches}))
}

fn tool_web_fetch(args: &Value) -> Result<Value, String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "web_fetch requires url".to_string())?;
    if let Some(text) = lookup_tool_stub_fetch(args, url) {
        return Ok(json!({"url": url, "content": text}));
    }
    let max_chars = args
        .get("max_chars")
        .and_then(Value::as_u64)
        .unwrap_or(6000) as usize;
    let body = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())?
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| err.to_string())?
        .text()
        .map_err(|err| err.to_string())?;
    let trimmed = trim_chars(&body, max_chars);
    Ok(json!({"url": url, "content": trimmed}))
}

fn tool_web_search(config: &Value, args: &Value) -> Result<Value, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| "web_search requires query".to_string())?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, 10) as usize;
    let provider = web_search_provider(config)?;
    if let Some(results) = lookup_tool_stub_search(args, query) {
        return Ok(json!({
            "query": query,
            "provider": provider.as_str(),
            "results": results
        }));
    }
    let results = match provider {
        WebSearchProvider::DuckDuckGoHtml => web_search_duckduckgo_html(query, limit)?,
        WebSearchProvider::BraveApi => web_search_brave_api(config, query, limit)?,
    };
    Ok(json!({
        "query": query,
        "provider": provider.as_str(),
        "results": results
    }))
}

fn tool_file_write(args: &Value) -> Result<Value, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "file_write requires path".to_string())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "file_write requires content".to_string())?;
    let resolved = expand_home(path);
    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::write(&resolved, content).map_err(|err| err.to_string())?;
    Ok(json!({"path": path, "bytes": content.len()}))
}

fn tool_file_edit(args: &Value) -> Result<Value, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "file_edit requires path".to_string())?;
    let find = args
        .get("find")
        .and_then(Value::as_str)
        .ok_or_else(|| "file_edit requires find".to_string())?;
    let replace = args
        .get("replace")
        .and_then(Value::as_str)
        .ok_or_else(|| "file_edit requires replace".to_string())?;
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let resolved = expand_home(path);
    let mut content = fs::read_to_string(&resolved).map_err(|err| err.to_string())?;
    let occurrences = content.matches(find).count();
    if occurrences == 0 {
        return Err("file_edit could not find target text".into());
    }
    content = if replace_all {
        content.replace(find, replace)
    } else {
        content.replacen(find, replace, 1)
    };
    fs::write(&resolved, content).map_err(|err| err.to_string())?;
    Ok(json!({"path": path, "replacements": if replace_all { occurrences } else { 1 }}))
}

fn tool_shell_exec(args: &Value) -> Result<Value, String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| "shell_exec requires command".to_string())?;
    let cwd = args.get("cwd").and_then(Value::as_str);
    let mut process = ProcessCommand::new("/bin/zsh");
    process.arg("-lc").arg(command);
    if let Some(cwd) = cwd {
        process.current_dir(expand_home(cwd));
    }
    let output = process.output().map_err(|err| err.to_string())?;
    Ok(json!({
        "command": command,
        "status": output.status.code().unwrap_or(-1),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    }))
}

fn spotlight_name_search(root: &Path, query: &str, kind: &str, limit: usize) -> Vec<String> {
    if !cfg!(target_os = "macos") || !root.exists() {
        return Vec::new();
    }
    let output = ProcessCommand::new("/usr/bin/mdfind")
        .arg("-onlyin")
        .arg(root)
        .arg("-name")
        .arg(query)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.exists() && path_kind_matches(path, kind))
        .take(limit)
        .map(|path| path.to_string_lossy().to_string())
        .collect()
}

fn path_kind_matches(path: &Path, kind: &str) -> bool {
    match kind {
        "file" => path.is_file(),
        "dir" => path.is_dir(),
        _ => path.exists(),
    }
}

fn path_matches_query(path: &Path, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    let display = path.to_string_lossy().to_lowercase();
    if name.contains(&query) || display.contains(&query) {
        return true;
    }
    let tokens = query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    !tokens.is_empty()
        && tokens
            .iter()
            .all(|token| name.contains(token) || display.contains(token))
}

fn tool_mcp_resource_read(args: &Value) -> Result<Value, String> {
    let server = args
        .get("server")
        .and_then(Value::as_str)
        .ok_or_else(|| "mcp_resource_read requires server".to_string())?;
    let uri = args
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| "mcp_resource_read requires uri".to_string())?;
    if let Some(value) = args
        .get("_stub")
        .and_then(|stub| stub.get("mcp_resource_read"))
        .and_then(|entries| entries.get(format!("{server}|{uri}")))
    {
        return Ok(json!({
            "server": server,
            "uri": uri,
            "content": value.clone(),
        }));
    }
    Err("mcp_resource_read is not yet connected to external MCP servers".into())
}

fn tool_task_note_write(config: &Value, args: &Value) -> Result<Value, String> {
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "task_note_write requires kind".to_string())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "task_note_write requires content".to_string())?;
    upsert_memory_artifact(
        config,
        "tasks",
        kind,
        content,
        &json!({"tool": "task_note_write"}),
    )?;
    Ok(json!({"kind": kind, "written": true}))
}

fn tool_agent_handoff_create(config: &Value, args: &Value) -> Result<Value, String> {
    let agent = args
        .get("agent")
        .and_then(Value::as_str)
        .ok_or_else(|| "agent_handoff_create requires agent".to_string())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "agent_handoff_create requires content".to_string())?;
    let artifact_kind = format!("handoff:{agent}");
    upsert_memory_artifact(
        config,
        "agents",
        &artifact_kind,
        content,
        &json!({"tool": "agent_handoff_create", "agent": agent}),
    )?;
    Ok(json!({"agent": agent, "written": true}))
}

fn memory_connection(config: &Value) -> Result<Connection, String> {
    let path = expand_home(&memory_path(config));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let conn = Connection::open(path).map_err(|err| err.to_string())?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS events (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          plugin TEXT NOT NULL,
          kind TEXT NOT NULL,
          key TEXT NOT NULL DEFAULT '',
          value_json TEXT NOT NULL,
          created_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_events_plugin_kind_created
          ON events(plugin, kind, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_events_plugin_key_created
          ON events(plugin, key, created_at DESC);

        CREATE TABLE IF NOT EXISTS state (
          plugin TEXT NOT NULL,
          key TEXT NOT NULL,
          value_json TEXT NOT NULL,
          updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
          PRIMARY KEY(plugin, key)
        );

        CREATE TABLE IF NOT EXISTS artifacts (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          plugin TEXT NOT NULL,
          kind TEXT NOT NULL,
          content TEXT NOT NULL,
          source_json TEXT NOT NULL DEFAULT '{}',
          active INTEGER NOT NULL DEFAULT 1,
          created_at INTEGER NOT NULL DEFAULT (unixepoch()),
          updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_artifacts_plugin_kind_active
          ON artifacts(plugin, kind, active, updated_at DESC, id DESC);
        ",
    )
    .map_err(|err| err.to_string())?;
    Ok(conn)
}

pub fn upsert_memory_artifact(
    config: &Value,
    plugin: &str,
    kind: &str,
    content: &str,
    source: &Value,
) -> Result<(), String> {
    let conn = memory_connection(config)?;
    conn.execute(
        "UPDATE artifacts SET active = 0 WHERE plugin = ?1 AND kind = ?2 AND active = 1",
        params![plugin, kind],
    )
    .map_err(|err| err.to_string())?;
    conn.execute(
        "INSERT INTO artifacts(plugin, kind, content, source_json, active, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, unixepoch(), unixepoch())",
        params![plugin, kind, content, source.to_string()],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

fn memory_path(config: &Value) -> String {
    value_or_env(
        config,
        "NOODLE_MEMORY_DB",
        "memory.path",
        &default_memory_path(),
    )
}

fn default_memory_path() -> String {
    env::var("NOODLE_MEMORY_DB").unwrap_or_else(|_| "~/.noodle/memory.db".into())
}

fn value_or_env(config: &Value, env_name: &str, key: &str, default: &str) -> String {
    if !env_name.is_empty() {
        if let Ok(value) = env::var(env_name) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    match lookup(config, key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => {
            if *value {
                "1".into()
            } else {
                "0".into()
            }
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<Vec<_>>()
            .join(" "),
        Some(Value::Null) | None => default.into(),
        Some(other) => other.to_string(),
    }
}

fn lookup<'a>(config: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = config;
    for part in key.split('.') {
        if part.is_empty() {
            continue;
        }
        current = current.get(part)?;
    }
    Some(current)
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::{
        enabled_plugin_manifests, exported_mcp_tools, plugin_matches_request,
        registered_slash_command_names, slash_command_name, tool_glob, tools_for_plugin,
        tool_path_search, wildcard_match,
    };
    use serde_json::json;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn enabled_plugin_manifests_follow_config_order() {
        let config = json!({
            "plugins": {
                "order": ["typos", "chat"]
            }
        });
        let manifests = enabled_plugin_manifests(&config);
        let ids = manifests
            .iter()
            .map(|manifest| manifest.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["typos", "chat"]);
    }

    #[test]
    fn manifest_matching_is_event_and_input_aware() {
        let config = json!({
            "plugins": {
                "chat": {
                    "prefix": ","
                }
            }
        });
        let manifests = enabled_plugin_manifests(&config);
        let chat = manifests
            .iter()
            .find(|manifest| manifest.id == "chat")
            .unwrap();
        let typos = manifests
            .iter()
            .find(|manifest| manifest.id == "typos")
            .unwrap();
        assert!(plugin_matches_request(
            &config,
            chat,
            "command_not_found",
            "oo hello"
        ));
        assert!(plugin_matches_request(
            &config,
            chat,
            "command_not_found",
            ", hello"
        ));
        assert!(!plugin_matches_request(
            &config,
            chat,
            "command_not_found",
            "die"
        ));
        assert!(plugin_matches_request(
            &config,
            typos,
            "command_not_found",
            "die"
        ));
        assert!(plugin_matches_request(
            &config,
            typos,
            "command_error",
            "bad command"
        ));
        assert!(!plugin_matches_request(
            &config,
            typos,
            "shell_started",
            "bad command"
        ));
    }

    #[test]
    fn exported_mcp_tools_follow_plugin_exports() {
        let config = json!({
            "plugins": {
                "order": ["chat", "typos"],
                "chat": {
                    "exports_tools": ["chat.send"]
                },
                "typos": {
                    "exports_tools": []
                }
            }
        });
        let tools = exported_mcp_tools(&config);
        let names = tools.iter().map(|tool| tool.name).collect::<Vec<_>>();
        assert_eq!(names, vec!["chat.send"]);
    }

    #[test]
    fn plugin_tools_can_be_disabled_with_boolean_availability() {
        let config = json!({
            "plugins": {
                "chat": {
                    "tool_availability": {
                        "file_read": false,
                        "glob": false
                    }
                }
            }
        });
        let tools = tools_for_plugin(&config, "chat");
        let names = tools.iter().map(|tool| tool.name).collect::<Vec<_>>();
        assert!(!names.contains(&"file_read"));
        assert!(!names.contains(&"glob"));
        assert!(names.contains(&"grep"));
    }

    #[test]
    fn plugin_tools_can_be_enabled_with_boolean_availability() {
        let config = json!({
            "plugins": {
                "typos": {
                    "tool_availability": {
                        "file_read": true
                    }
                }
            }
        });
        let tools = tools_for_plugin(&config, "typos");
        let names = tools.iter().map(|tool| tool.name).collect::<Vec<_>>();
        assert_eq!(names, vec!["file_read"]);
    }

    #[test]
    fn registered_slash_commands_follow_enabled_plugins() {
        let config = json!({
            "plugins": {
                "order": ["todo", "chat"]
            }
        });
        assert_eq!(registered_slash_command_names(&config), vec!["todo"]);
    }

    #[test]
    fn slash_command_matching_is_explicit() {
        let config = json!({
            "plugins": {
                "order": ["todo", "chat"]
            }
        });
        let manifests = enabled_plugin_manifests(&config);
        let todo = manifests
            .iter()
            .find(|manifest| manifest.id == "todo")
            .unwrap();
        assert_eq!(slash_command_name("/todo add ship it").as_deref(), Some("todo"));
        assert!(plugin_matches_request(
            &config,
            todo,
            "slash_command",
            "/todo add ship it"
        ));
        assert!(!plugin_matches_request(
            &config,
            todo,
            "command_not_found",
            "/todo add ship it"
        ));
        assert!(!plugin_matches_request(
            &config,
            todo,
            "slash_command",
            "/unknown test"
        ));
    }

    #[test]
    fn wildcard_match_treats_globstar_as_zero_or_more_segments() {
        assert!(wildcard_match(
            "**/claude-code-main/**/README*",
            "claude-code-main/README.md"
        ));
        assert!(wildcard_match(
            "**/claude-code-main/**/README*",
            "/tmp/claude-code-main/README.md"
        ));
        assert!(wildcard_match(
            "**/claude-code-main/**/[Rr][Ee][Aa][Dd][Mm][Ee]*",
            "/tmp/claude-code-main/README.md"
        ));
    }

    #[test]
    fn tool_glob_matches_root_relative_paths_with_globstar_patterns() {
        let root = temp_dir("noodle-tool-glob");
        let repo = root.join("claude-code-main");
        fs::create_dir_all(&repo).unwrap();
        let readme = repo.join("README.md");
        fs::write(&readme, "snapshot").unwrap();

        let result = tool_glob(&json!({
            "root": root.to_string_lossy().to_string(),
            "pattern": "**/claude-code-main/**/README*",
            "limit": 50
        }))
        .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert!(
            matches
                .iter()
                .any(|item| item.as_str() == Some(readme.to_string_lossy().as_ref())),
            "expected {:?} in {:?}",
            readme,
            matches
        );

        let result = tool_glob(&json!({
            "root": root.to_string_lossy().to_string(),
            "pattern": "**/claude-code-main/**/[Rr][Ee][Aa][Dd][Mm][Ee]*",
            "limit": 50
        }))
        .unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(
            matches
                .iter()
                .any(|item| item.as_str() == Some(readme.to_string_lossy().as_ref())),
            "expected {:?} in {:?}",
            readme,
            matches
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tool_file_read_normalizes_bom_and_crlf_and_reports_canonical_path() {
        let root = temp_dir("noodle-tool-read");
        let path = root.join("README.md");
        fs::write(&path, "\u{feff}line one\r\nline two\r\n").unwrap();

        let result = super::tool_file_read(&json!({
            "path": path.to_string_lossy().to_string()
        }))
        .unwrap();

        assert_eq!(result["content"], "line one\nline two\n");
        assert_eq!(
            result["path"],
            fs::canonicalize(&path)
                .unwrap()
                .to_string_lossy()
                .to_string()
        );
        assert!(result["mtime"].as_i64().is_some());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tool_path_search_finds_matching_directories_and_files() {
        let root = temp_dir("noodle-tool-path-search");
        let village = root.join("Tobacco Branch Village POA");
        fs::create_dir_all(&village).unwrap();
        let agenda = village.join("agenda.txt");
        fs::write(&agenda, "meeting").unwrap();

        let dir_result = tool_path_search(&json!({
            "root": root.to_string_lossy().to_string(),
            "query": "tobacco branch village",
            "kind": "dir",
            "limit": 10
        }))
        .unwrap();
        assert!(
            dir_result["matches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str() == Some(village.to_string_lossy().as_ref()))
        );

        let file_result = tool_path_search(&json!({
            "root": root.to_string_lossy().to_string(),
            "query": "agenda",
            "kind": "file",
            "limit": 10
        }))
        .unwrap();
        assert!(
            file_result["matches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str() == Some(agenda.to_string_lossy().as_ref()))
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn tool_path_search_skips_permission_denied_directories() {
        let root = temp_dir("noodle-tool-path-search-perms");
        let blocked = root.join("blocked");
        let visible = root.join("visible");
        fs::create_dir_all(&blocked).unwrap();
        fs::create_dir_all(&visible).unwrap();
        let target = visible.join("visible match.txt");
        fs::write(&target, "ok").unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

        let result = tool_path_search(&json!({
            "root": root.to_string_lossy().to_string(),
            "query": "visible match",
            "kind": "file",
            "limit": 10
        }))
        .unwrap();

        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            result["matches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str() == Some(target.to_string_lossy().as_ref()))
        );
        fs::remove_dir_all(root).unwrap();
    }
}

fn visit_paths(root: &Path, visitor: &mut dyn FnMut(&Path)) -> Result<(), String> {
    if root.is_file() {
        visitor(root);
        return Ok(());
    }
    if !root.exists() {
        return Ok(());
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if is_permission_denied(&err) => return Ok(()),
        Err(err) => return Err(err.to_string()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) if is_permission_denied(&err) => continue,
            Err(err) => return Err(err.to_string()),
        };
        let path = entry.path();
        if path.is_dir() {
            visit_paths(&path, visitor)?;
        } else if path.is_file() {
            visitor(&path);
        }
    }
    Ok(())
}

fn visit_nodes(root: &Path, visitor: &mut dyn FnMut(&Path)) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    visitor(root);
    if root.is_file() {
        return Ok(());
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if is_permission_denied(&err) => return Ok(()),
        Err(err) => return Err(err.to_string()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) if is_permission_denied(&err) => continue,
            Err(err) => return Err(err.to_string()),
        };
        visit_nodes(&entry.path(), visitor)?;
    }
    Ok(())
}

fn is_permission_denied(err: &std::io::Error) -> bool {
    matches!(err.kind(), std::io::ErrorKind::PermissionDenied)
        || matches!(err.raw_os_error(), Some(1 | 13))
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = normalize_glob_path(pattern);
    let text = normalize_glob_path(text);
    if pattern.contains('/') {
        let pattern_segments = split_glob_segments(&pattern);
        let text_segments = split_glob_segments(&text);
        wildcard_match_segments(&pattern_segments, &text_segments)
    } else {
        wildcard_match_component(pattern.as_bytes(), text.as_bytes())
    }
}

fn wildcard_match_segments(pattern: &[&str], text: &[&str]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    if pattern[0] == "**" {
        return wildcard_match_segments(&pattern[1..], text)
            || (!text.is_empty() && wildcard_match_segments(pattern, &text[1..]));
    }
    !text.is_empty()
        && wildcard_match_component(pattern[0].as_bytes(), text[0].as_bytes())
        && wildcard_match_segments(&pattern[1..], &text[1..])
}

fn wildcard_match_component(pattern: &[u8], text: &[u8]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    match pattern[0] {
        b'*' => {
            wildcard_match_component(&pattern[1..], text)
                || (!text.is_empty() && wildcard_match_component(pattern, &text[1..]))
        }
        b'?' => !text.is_empty() && wildcard_match_component(&pattern[1..], &text[1..]),
        b'[' => {
            if text.is_empty() {
                return false;
            }
            if let Some((matched, consumed)) = match_bracket_class(pattern, text[0]) {
                matched && wildcard_match_component(&pattern[consumed..], &text[1..])
            } else {
                false
            }
        }
        c => !text.is_empty() && c == text[0] && wildcard_match_component(&pattern[1..], &text[1..]),
    }
}

fn normalize_glob_path(input: &str) -> String {
    input.replace('\\', "/")
}

fn split_glob_segments(input: &str) -> Vec<&str> {
    input.split('/').filter(|segment| !segment.is_empty()).collect()
}

fn match_bracket_class(pattern: &[u8], value: u8) -> Option<(bool, usize)> {
    if pattern.first() != Some(&b'[') {
        return None;
    }
    let mut index = 1usize;
    let mut negate = false;
    if index < pattern.len() && (pattern[index] == b'!' || pattern[index] == b'^') {
        negate = true;
        index += 1;
    }
    let mut matched = false;
    while index < pattern.len() {
        if pattern[index] == b']' {
            let result = if negate { !matched } else { matched };
            return Some((result, index + 1));
        }
        if index + 2 < pattern.len() && pattern[index + 1] == b'-' && pattern[index + 2] != b']' {
            let start = pattern[index];
            let end = pattern[index + 2];
            if start <= value && value <= end {
                matched = true;
            }
            index += 3;
            continue;
        }
        if pattern[index] == value {
            matched = true;
        }
        index += 1;
    }
    None
}

fn trim_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect::<String>()
}

fn normalize_file_content(raw: &str) -> String {
    let without_bom = if raw.starts_with('\u{feff}') {
        &raw['\u{feff}'.len_utf8()..]
    } else {
        raw
    };
    without_bom.replace("\r\n", "\n")
}

fn url_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSearchProvider {
    DuckDuckGoHtml,
    BraveApi,
}

impl WebSearchProvider {
    fn as_str(self) -> &'static str {
        match self {
            Self::DuckDuckGoHtml => "duckduckgo_html",
            Self::BraveApi => "brave_api",
        }
    }
}

fn web_search_provider(config: &Value) -> Result<WebSearchProvider, String> {
    match value_or_env(
        config,
        "NOODLE_SEARCH_PROVIDER",
        "search.provider",
        "duckduckgo_html",
    )
    .as_str()
    {
        "duckduckgo_html" => Ok(WebSearchProvider::DuckDuckGoHtml),
        "brave_api" => Ok(WebSearchProvider::BraveApi),
        other => Err(format!(
            "unsupported web_search provider: {other} (expected duckduckgo_html or brave_api)"
        )),
    }
}

fn web_search_duckduckgo_html(query: &str, limit: usize) -> Result<Vec<Value>, String> {
    let encoded = url_encode(query);
    let url = format!("https://duckduckgo.com/html/?q={encoded}");
    let body = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())?
        .get(&url)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| err.to_string())?
        .text()
        .map_err(|err| err.to_string())?;
    Ok(parse_duckduckgo_results(&body, limit))
}

fn web_search_brave_api(config: &Value, query: &str, limit: usize) -> Result<Vec<Value>, String> {
    let api_key = brave_search_api_key(config)?;
    let base_url = value_or_env(
        config,
        "NOODLE_BRAVE_SEARCH_BASE_URL",
        "search.brave.base_url",
        "https://api.search.brave.com/res/v1/web/search",
    );
    let country = value_or_env(config, "", "search.brave.country", "us");
    let search_lang = value_or_env(config, "", "search.brave.search_lang", "en");
    let count = limit.to_string();
    let body = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())?
        .get(&base_url)
        .header("Accept", "application/json")
        .header("Accept-Encoding", "gzip")
        .header("X-Subscription-Token", api_key)
        .query(&[
            ("q", query),
            ("count", count.as_str()),
            ("country", country.as_str()),
            ("search_lang", search_lang.as_str()),
        ])
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| err.to_string())?
        .json::<Value>()
        .map_err(|err| err.to_string())?;
    Ok(parse_brave_results(&body, limit))
}

fn brave_search_api_key(config: &Value) -> Result<String, String> {
    for env_name in ["NOODLE_BRAVE_SEARCH_API_KEY", "BRAVE_SEARCH_API_KEY"] {
        if let Ok(value) = env::var(env_name) {
            if !value.trim().is_empty() {
                return Ok(value);
            }
        }
    }
    let configured = lookup(config, "search.brave.api_key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if configured.is_empty() {
        Err("search.brave.api_key or BRAVE_SEARCH_API_KEY is required when search.provider is brave_api".into())
    } else {
        Ok(configured)
    }
}

fn parse_duckduckgo_results(body: &str, limit: usize) -> Vec<Value> {
    let mut results = Vec::new();
    for line in body.lines() {
        if results.len() >= limit {
            break;
        }
        if let Some(start) = line.find("result__a\" href=\"") {
            let href_start = start + "result__a\" href=\"".len();
            if let Some(href_end) = line[href_start..].find('"') {
                let href = &line[href_start..href_start + href_end];
                let title_start = line[href_start + href_end..]
                    .find('>')
                    .map(|n| href_start + href_end + n + 1);
                let title_end = title_start
                    .and_then(|start_index| line[start_index..].find('<').map(|n| start_index + n));
                if let (Some(title_start), Some(title_end)) = (title_start, title_end) {
                    let title = html_unescape(&line[title_start..title_end]);
                    results.push(json!({"title": title, "url": href}));
                }
            }
        }
    }
    results
}

fn parse_brave_results(body: &Value, limit: usize) -> Vec<Value> {
    body.get("web")
        .and_then(|web| web.get("results"))
        .and_then(Value::as_array)
        .map(|items| {
            items.iter()
                .filter_map(|item| {
                    let title = item.get("title").and_then(Value::as_str)?;
                    let url = item.get("url").and_then(Value::as_str)?;
                    Some(json!({
                        "title": html_unescape(title),
                        "url": url
                    }))
                })
                .take(limit)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn html_unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn lookup_tool_stub_fetch(args: &Value, url: &str) -> Option<String> {
    args.get("_stub")
        .and_then(|stub| stub.get("web_fetch"))
        .and_then(|entries| entries.get(url))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn lookup_tool_stub_search(args: &Value, query: &str) -> Option<Vec<Value>> {
    args.get("_stub")
        .and_then(|stub| stub.get("web_search"))
        .and_then(|entries| entries.get(query))
        .and_then(Value::as_array)
        .cloned()
}

pub fn tool_definition_by_name(name: &str) -> Option<ToolDefinition> {
    builtin_tool_definitions()
        .into_iter()
        .find(|tool| tool.name == name)
}

pub fn permission_decision_for_tool(config: &Value, tool_name: &str) -> ToolPermissionDecision {
    if let Some(value) = lookup(config, &format!("permissions.tools.{tool_name}")) {
        return parse_permission_decision(value);
    }
    let class = tool_definition_by_name(tool_name)
        .map(|tool| tool.permission)
        .unwrap_or(ToolPermissionClass::External);
    if let Some(value) = lookup(config, &format!("permissions.classes.{}", class.as_str())) {
        return parse_permission_decision(value);
    }
    match class {
        ToolPermissionClass::ReadOnly | ToolPermissionClass::NetworkRead => {
            ToolPermissionDecision::Allow
        }
        ToolPermissionClass::LocalWrite
        | ToolPermissionClass::ShellExec
        | ToolPermissionClass::InteractiveShell
        | ToolPermissionClass::External => ToolPermissionDecision::Ask,
    }
}

fn parse_permission_decision(value: &Value) -> ToolPermissionDecision {
    match value {
        Value::String(text) if text.eq_ignore_ascii_case("allow") => ToolPermissionDecision::Allow,
        Value::String(text) if text.eq_ignore_ascii_case("deny") => ToolPermissionDecision::Deny,
        Value::String(text) if text.eq_ignore_ascii_case("ask") => ToolPermissionDecision::Ask,
        Value::Bool(true) => ToolPermissionDecision::Allow,
        Value::Bool(false) => ToolPermissionDecision::Deny,
        _ => ToolPermissionDecision::Ask,
    }
}

pub fn active_artifact_content(
    config: &Value,
    plugin: &str,
    kind: &str,
) -> Result<Option<String>, String> {
    let conn = memory_connection(config)?;
    conn.query_row(
        "SELECT content
         FROM artifacts
         WHERE plugin = ?1 AND kind = ?2 AND active = 1
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![plugin, kind],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|err| err.to_string())
}

pub fn deactivate_artifact(config: &Value, plugin: &str, kind: &str) -> Result<(), String> {
    let conn = memory_connection(config)?;
    conn.execute(
        "UPDATE artifacts SET active = 0 WHERE plugin = ?1 AND kind = ?2 AND active = 1",
        params![plugin, kind],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}
