use crate::actions::DaemonAction;
use crate::tooling::{
    plugin_order, registered_slash_command_names, slash_command_definition, tools_for_plugin,
};
use crate::{expand_home, lookup};
use serde_json::{Map, Value};
use std::env;
use std::fs;
use std::path::PathBuf;

const UTILS_PLUGIN: &str = "utils";

pub fn handle_utils_command(config: &Value, raw_input: &str) -> Result<Value, String> {
    match parse_utils_command(raw_input)? {
        UtilsCommand::Help => Ok(DaemonAction::Message {
            plugin: UTILS_PLUGIN.into(),
            message: render_help(config),
        }
        .into_value()),
        UtilsCommand::Status => Ok(DaemonAction::Message {
            plugin: UTILS_PLUGIN.into(),
            message: render_status(config),
        }
        .into_value()),
        UtilsCommand::Reload => Ok(DaemonAction::ReloadRuntime {
            plugin: UTILS_PLUGIN.into(),
            message: "Reloaded noodle runtime config.".into(),
        }
        .into_value()),
        UtilsCommand::Config(command) => Ok(DaemonAction::Message {
            plugin: UTILS_PLUGIN.into(),
            message: handle_config_command(config, command)?,
        }
        .into_value()),
    }
}

enum UtilsCommand {
    Help,
    Status,
    Reload,
    Config(ConfigCommand),
}

enum ConfigCommand {
    Help,
    Show { key: Option<String> },
    Get { key: String },
    Set { key: String, value: String },
    Unset { key: String },
}

fn parse_utils_command(raw_input: &str) -> Result<UtilsCommand, String> {
    let trimmed = raw_input.trim();
    if trimmed == "/help" {
        return Ok(UtilsCommand::Help);
    }
    if trimmed == "/status" {
        return Ok(UtilsCommand::Status);
    }
    if trimmed == "/reload" {
        return Ok(UtilsCommand::Reload);
    }
    if let Some(rest) = trimmed.strip_prefix("/config") {
        return Ok(UtilsCommand::Config(parse_config_command(rest.trim())?));
    }
    Err(format!("Unknown utils command: {}", trimmed))
}

fn parse_config_command(rest: &str) -> Result<ConfigCommand, String> {
    if rest.is_empty() {
        return Ok(ConfigCommand::Help);
    }
    let (subcommand, remainder) = if let Some(index) = rest.find(char::is_whitespace) {
        (&rest[..index], rest[index..].trim())
    } else {
        (rest, "")
    };
    match subcommand {
        "help" => Ok(ConfigCommand::Help),
        "show" => Ok(ConfigCommand::Show {
            key: if remainder.is_empty() {
                None
            } else {
                Some(remainder.to_string())
            },
        }),
        "get" => {
            if remainder.is_empty() {
                Err("Usage: /config get <key>".into())
            } else {
                Ok(ConfigCommand::Get {
                    key: remainder.to_string(),
                })
            }
        }
        "set" => {
            let Some(index) = remainder.find(char::is_whitespace) else {
                return Err("Usage: /config set <key> <value>".into());
            };
            let key = remainder[..index].trim();
            let value = remainder[index..].trim();
            if key.is_empty() || value.is_empty() {
                return Err("Usage: /config set <key> <value>".into());
            }
            Ok(ConfigCommand::Set {
                key: key.to_string(),
                value: value.to_string(),
            })
        }
        "unset" => {
            if remainder.is_empty() {
                Err("Usage: /config unset <key>".into())
            } else {
                Ok(ConfigCommand::Unset {
                    key: remainder.to_string(),
                })
            }
        }
        _ => Err(format!(
            "Unknown config command: {}.\n{}",
            subcommand,
            config_help_text()
        )),
    }
}

fn render_help(config: &Value) -> String {
    let mut lines = vec!["Slash commands:".to_string()];
    for command in registered_slash_command_names(config) {
        if let Some(definition) = slash_command_definition(&command) {
            lines.push(format!(
                "/{} - {}",
                definition.name, definition.description
            ));
            lines.push(format!("  {}", definition.usage));
        }
    }
    lines.join("\n")
}

fn render_status(config: &Value) -> String {
    let config_path = resolved_config_path(config);
    let memory_path = config
        .get("memory")
        .and_then(|value| value.get("path"))
        .and_then(Value::as_str)
        .map(expand_home)
        .unwrap_or_else(|| expand_home("~/.noodle/memory.db"));
    let plugins = plugin_order(config)
        .into_iter()
        .map(|plugin| format!("- {}", plugin))
        .collect::<Vec<_>>()
        .join("\n");
    let commands = registered_slash_command_names(config)
        .into_iter()
        .map(|name| format!("/{}", name))
        .collect::<Vec<_>>()
        .join(" ");
    let chat_prefix = lookup(config, "plugins.chat.prefix")
        .and_then(Value::as_str)
        .unwrap_or(",");
    let chat_tool_count = tools_for_plugin(config, "chat").len();
    let permission_lines = [
        "read_only",
        "network_read",
        "local_write",
        "shell_exec",
        "interactive_shell",
        "external",
    ]
    .into_iter()
    .map(|key| {
        let value = lookup(config, &format!("permissions.classes.{}", key))
            .and_then(Value::as_str)
            .unwrap_or("unset");
        format!("- {}: {}", key, value)
    })
    .collect::<Vec<_>>()
    .join("\n");

    format!(
        "Noodle status\nConfig: {}\nMemory DB: {}\nChat prefix: {}\nChat tools: {}\nPlugins:\n{}\nSlash commands: {}\nPermissions:\n{}",
        config_path.display(),
        memory_path.display(),
        chat_prefix,
        chat_tool_count,
        plugins,
        commands,
        permission_lines
    )
}

fn handle_config_command(config: &Value, command: ConfigCommand) -> Result<String, String> {
    match command {
        ConfigCommand::Help => Ok(config_help_text()),
        ConfigCommand::Show { key } => {
            let path = resolved_config_path(config);
            let document = load_config_document(&path)?;
            if let Some(key) = key {
                let value = lookup(&document, &key)
                    .cloned()
                    .ok_or_else(|| format!("Config key not found: {}", key))?;
                Ok(serde_json::to_string_pretty(&value).map_err(|err| err.to_string())?)
            } else {
                Ok(format!(
                    "Config path: {}\n{}",
                    path.display(),
                    serde_json::to_string_pretty(&document).map_err(|err| err.to_string())?
                ))
            }
        }
        ConfigCommand::Get { key } => {
            let value = lookup(config, &key)
                .cloned()
                .ok_or_else(|| format!("Config key not found: {}", key))?;
            Ok(render_value_inline(&value))
        }
        ConfigCommand::Set { key, value } => {
            let path = resolved_config_path(config);
            let mut current = load_config_document(&path)?;
            let parsed = parse_config_value(&value);
            set_path_value(&mut current, &key, parsed)?;
            save_config_document(&path, &current)?;
            Ok(format!(
                "Updated {} in {}.\nNew value: {}",
                key,
                path.display(),
                render_value_inline(
                    lookup(&current, &key)
                        .ok_or_else(|| "config write failed".to_string())?
                )
            ))
        }
        ConfigCommand::Unset { key } => {
            let path = resolved_config_path(config);
            let mut current = load_config_document(&path)?;
            remove_path_value(&mut current, &key)?;
            save_config_document(&path, &current)?;
            Ok(format!("Removed {} from {}.", key, path.display()))
        }
    }
}

fn resolved_config_path(config: &Value) -> PathBuf {
    let path = lookup(config, "_meta.config_path")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| env::var("NOODLE_CONFIG").unwrap_or_else(|_| "~/.noodle/config.json".into()));
    expand_home(&path)
}

fn load_config_document(path: &PathBuf) -> Result<Value, String> {
    fs::read_to_string(path)
        .map_err(|err| err.to_string())
        .and_then(|body| serde_json::from_str(&body).map_err(|err| err.to_string()))
}

fn save_config_document(path: &PathBuf, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(value).map_err(|err| err.to_string())?
        ),
    )
    .map_err(|err| err.to_string())
}

fn parse_config_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn set_path_value(root: &mut Value, key: &str, value: Value) -> Result<(), String> {
    let segments = split_config_key(key)?;
    let mut current = root;
    for segment in &segments[..segments.len().saturating_sub(1)] {
        if current.get(segment).is_none() {
            current
                .as_object_mut()
                .ok_or_else(|| format!("Cannot write into non-object path: {}", key))?
                .insert(segment.clone(), Value::Object(Map::new()));
        }
        current = current
            .get_mut(segment)
            .ok_or_else(|| format!("Cannot descend into config path: {}", key))?;
        if !current.is_object() {
            return Err(format!("Cannot write into non-object path: {}", key));
        }
    }
    let last = segments
        .last()
        .ok_or_else(|| "Config key cannot be empty".to_string())?;
    current
        .as_object_mut()
        .ok_or_else(|| format!("Cannot set config key: {}", key))?
        .insert(last.clone(), value);
    Ok(())
}

fn remove_path_value(root: &mut Value, key: &str) -> Result<(), String> {
    let segments = split_config_key(key)?;
    let mut current = root;
    for segment in &segments[..segments.len().saturating_sub(1)] {
        current = current
            .get_mut(segment)
            .ok_or_else(|| format!("Config key not found: {}", key))?;
    }
    let last = segments
        .last()
        .ok_or_else(|| "Config key cannot be empty".to_string())?;
    let removed = current
        .as_object_mut()
        .ok_or_else(|| format!("Config key not found: {}", key))?
        .remove(last);
    if removed.is_none() {
        return Err(format!("Config key not found: {}", key));
    }
    Ok(())
}

fn split_config_key(key: &str) -> Result<Vec<String>, String> {
    let parts = key
        .split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        Err("Config key cannot be empty".into())
    } else {
        Ok(parts)
    }
}

fn render_value_inline(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn config_help_text() -> String {
    [
        "Config commands:",
        "/config help",
        "/config show",
        "/config show <key>",
        "/config get <key>",
        "/config set <key> <value>",
        "/config unset <key>",
    ]
    .join("\n")
}
