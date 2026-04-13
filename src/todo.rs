use crate::actions::DaemonAction;
use crate::{
    PluginId, memory_after_event, memory_append_event, memory_get_state,
    memory_increment_state_counter, memory_set_state, memory_upsert_artifact,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const TODO_PLUGIN: &str = "todo";
const TODO_STATE_KEY: &str = "items";
const TODO_ARTIFACT_KIND: &str = "list";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoItem {
    id: i64,
    text: String,
    #[serde(default)]
    partial: bool,
    done: bool,
    created_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
}

pub fn handle_todo_command(config: &Value, raw_input: &str) -> Result<Value, String> {
    let action = match parse_todo_command(raw_input)? {
        TodoCommand::Help => DaemonAction::Message {
            plugin: TODO_PLUGIN.into(),
            message: todo_help_text(),
        },
        TodoCommand::List => DaemonAction::Message {
            plugin: TODO_PLUGIN.into(),
            message: render_todo_list(&load_todos(config)?),
        },
        TodoCommand::Add { text } => {
            let mut items = load_todos(config)?;
            let id = memory_increment_state_counter(config, TODO_PLUGIN, "next_id")?;
            let now = unix_timestamp();
            let item = TodoItem {
                id,
                text: text.clone(),
                partial: false,
                done: false,
                created_at: now,
                updated_at: now,
                completed_at: None,
            };
            items.push(item.clone());
            save_todos(config, &items)?;
            record_todo_event(
                config,
                "add",
                &json!({"id": id, "text": text, "done": false}),
            )?;
            DaemonAction::Message {
                plugin: TODO_PLUGIN.into(),
                message: format!("Added todo #{}: {}", item.id, item.text),
            }
        }
        TodoCommand::Done { id } => {
            let mut items = load_todos(config)?;
            let item = find_todo_mut(&mut items, id)?;
            if item.done {
                return Ok(DaemonAction::Message {
                    plugin: TODO_PLUGIN.into(),
                    message: format!("Todo #{} is already done.", item.id),
                }
                .into_value());
            }
            let now = unix_timestamp();
            item.partial = false;
            item.done = true;
            item.updated_at = now;
            item.completed_at = Some(now);
            let text = item.text.clone();
            save_todos(config, &items)?;
            record_todo_event(
                config,
                "done",
                &json!({"id": id, "text": text, "partial": false, "done": true}),
            )?;
            DaemonAction::Message {
                plugin: TODO_PLUGIN.into(),
                message: format!("Completed todo #{}: {}", id, text),
            }
        }
        TodoCommand::Partial { id } => {
            let mut items = load_todos(config)?;
            let item = find_todo_mut(&mut items, id)?;
            if item.partial {
                return Ok(DaemonAction::Message {
                    plugin: TODO_PLUGIN.into(),
                    message: format!("Todo #{} is already partially done.", item.id),
                }
                .into_value());
            }
            item.partial = true;
            item.done = false;
            item.updated_at = unix_timestamp();
            item.completed_at = None;
            let text = item.text.clone();
            save_todos(config, &items)?;
            record_todo_event(
                config,
                "partial",
                &json!({"id": id, "text": text, "partial": true, "done": false}),
            )?;
            DaemonAction::Message {
                plugin: TODO_PLUGIN.into(),
                message: format!("Marked todo #{} as partial: {}", id, text),
            }
        }
        TodoCommand::Reopen { id } => {
            let mut items = load_todos(config)?;
            let item = find_todo_mut(&mut items, id)?;
            if !item.done && !item.partial {
                return Ok(DaemonAction::Message {
                    plugin: TODO_PLUGIN.into(),
                    message: format!("Todo #{} is already open.", item.id),
                }
                .into_value());
            }
            item.partial = false;
            item.done = false;
            item.updated_at = unix_timestamp();
            item.completed_at = None;
            let text = item.text.clone();
            save_todos(config, &items)?;
            record_todo_event(
                config,
                "reopen",
                &json!({"id": id, "text": text, "partial": false, "done": false}),
            )?;
            DaemonAction::Message {
                plugin: TODO_PLUGIN.into(),
                message: format!("Reopened todo #{}: {}", id, text),
            }
        }
        TodoCommand::Remove { id } => {
            let mut items = load_todos(config)?;
            let index = items
                .iter()
                .position(|item| item.id == id)
                .ok_or_else(|| format!("Todo #{} does not exist.", id))?;
            let item = items.remove(index);
            save_todos(config, &items)?;
            record_todo_event(
                config,
                "remove",
                &json!({"id": id, "text": item.text, "done": item.done}),
            )?;
            DaemonAction::Message {
                plugin: TODO_PLUGIN.into(),
                message: format!("Removed todo #{}: {}", item.id, item.text),
            }
        }
        TodoCommand::Show { id } => {
            let items = load_todos(config)?;
            let item = items
                .iter()
                .find(|item| item.id == id)
                .ok_or_else(|| format!("Todo #{} does not exist.", id))?;
            DaemonAction::Message {
                plugin: TODO_PLUGIN.into(),
                message: render_todo_detail(item),
            }
        }
        TodoCommand::ClearDone => {
            let mut items = load_todos(config)?;
            let removed = items.iter().filter(|item| item.done).count();
            if removed == 0 {
                return Ok(DaemonAction::Message {
                    plugin: TODO_PLUGIN.into(),
                    message: "No completed todos to clear.".into(),
                }
                .into_value());
            }
            items.retain(|item| !item.done);
            save_todos(config, &items)?;
            record_todo_event(config, "clear_done", &json!({"removed": removed}))?;
            DaemonAction::Message {
                plugin: TODO_PLUGIN.into(),
                message: format!("Cleared {} completed todo(s).", removed),
            }
        }
    };
    Ok(action.into_value())
}

#[derive(Debug, Clone)]
enum TodoCommand {
    Help,
    List,
    Add { text: String },
    Done { id: i64 },
    Partial { id: i64 },
    Reopen { id: i64 },
    Remove { id: i64 },
    Show { id: i64 },
    ClearDone,
}

fn parse_todo_command(raw_input: &str) -> Result<TodoCommand, String> {
    let trimmed = raw_input.trim();
    let rest = trimmed
        .strip_prefix("/todo")
        .ok_or_else(|| "todo commands must start with /todo".to_string())?
        .trim();
    if rest.is_empty() {
        return Ok(TodoCommand::List);
    }

    let (subcommand, remainder) = if let Some(index) = rest.find(char::is_whitespace) {
        (&rest[..index], rest[index..].trim())
    } else {
        (rest, "")
    };

    match subcommand {
        "list" => Ok(TodoCommand::List),
        "help" => Ok(TodoCommand::Help),
        "add" => {
            if remainder.is_empty() {
                Err("Usage: /todo add <task>".into())
            } else {
                Ok(TodoCommand::Add {
                    text: remainder.to_string(),
                })
            }
        }
        "/" | "partial" => {
            parse_todo_id_command(remainder, |id| TodoCommand::Partial { id }, "partial")
        }
        "x" => parse_todo_id_command(remainder, |id| TodoCommand::Done { id }, "x"),
        "done" => parse_todo_id_command(remainder, |id| TodoCommand::Done { id }, "done"),
        "reopen" => parse_todo_id_command(remainder, |id| TodoCommand::Reopen { id }, "reopen"),
        "remove" | "rm" => {
            parse_todo_id_command(remainder, |id| TodoCommand::Remove { id }, "remove")
        }
        "show" => parse_todo_id_command(remainder, |id| TodoCommand::Show { id }, "show"),
        "clear-done" => Ok(TodoCommand::ClearDone),
        _ => Err(format!(
            "Unknown todo command: {}.\n{}",
            subcommand,
            todo_help_text()
        )),
    }
}

fn parse_todo_id_command(
    remainder: &str,
    build: impl FnOnce(i64) -> TodoCommand,
    name: &str,
) -> Result<TodoCommand, String> {
    let id = remainder
        .parse::<i64>()
        .map_err(|_| format!("Usage: /todo {} <id>", name))?;
    Ok(build(id))
}

fn load_todos(config: &Value) -> Result<Vec<TodoItem>, String> {
    let Some(value) = memory_get_state(config, TODO_PLUGIN, TODO_STATE_KEY)? else {
        return Ok(Vec::new());
    };
    serde_json::from_value(value).map_err(|err| err.to_string())
}

fn save_todos(config: &Value, items: &[TodoItem]) -> Result<(), String> {
    let value = serde_json::to_value(items).map_err(|err| err.to_string())?;
    memory_set_state(config, TODO_PLUGIN, TODO_STATE_KEY, &value)?;
    let summary = render_todo_list(items);
    memory_upsert_artifact(
        config,
        TODO_PLUGIN,
        TODO_ARTIFACT_KIND,
        &summary,
        &json!({
            "open": items.iter().filter(|item| !item.done).count(),
            "done": items.iter().filter(|item| item.done).count(),
            "count": items.len(),
        }),
    )?;
    Ok(())
}

fn record_todo_event(config: &Value, key: &str, value: &Value) -> Result<(), String> {
    memory_append_event(config, TODO_PLUGIN, "command", key, value)?;
    memory_after_event(config, PluginId::Todo, "command", 1, false)
}

fn find_todo_mut(items: &mut [TodoItem], id: i64) -> Result<&mut TodoItem, String> {
    items
        .iter_mut()
        .find(|item| item.id == id)
        .ok_or_else(|| format!("Todo #{} does not exist.", id))
}

fn render_todo_list(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "No todos yet.\nUse /todo add <task> to create one.".into();
    }

    let mut lines = Vec::new();
    let open_count = items
        .iter()
        .filter(|item| !item.done && !item.partial)
        .count();
    let partial_count = items
        .iter()
        .filter(|item| item.partial && !item.done)
        .count();
    let done_count = items.iter().filter(|item| item.done).count();
    lines.push(format!(
        "Todos: {} open, {} partial, {} done",
        open_count, partial_count, done_count
    ));

    for item in items.iter().filter(|item| !item.done && !item.partial) {
        lines.push(format!("#{} [ ] {}", item.id, item.text));
    }
    for item in items.iter().filter(|item| item.partial && !item.done) {
        lines.push(format!("#{} [/] {}", item.id, item.text));
    }
    for item in items.iter().filter(|item| item.done) {
        lines.push(format!("#{} [x] {}", item.id, item.text));
    }
    lines.join("\n")
}

fn render_todo_detail(item: &TodoItem) -> String {
    let mut lines = vec![
        format!("Todo #{}", item.id),
        format!(
            "Status: {}",
            if item.done {
                "done"
            } else if item.partial {
                "partial"
            } else {
                "open"
            }
        ),
        format!("Task: {}", item.text),
        format!("Created: {}", item.created_at),
        format!("Updated: {}", item.updated_at),
    ];
    if let Some(completed_at) = item.completed_at {
        lines.push(format!("Completed: {}", completed_at));
    }
    lines.join("\n")
}

fn todo_help_text() -> String {
    [
        "Todo commands:",
        "/todo list",
        "/todo add <task>",
        "/todo / <id>",
        "/todo x <id>",
        "/todo done <id>",
        "/todo reopen <id>",
        "/todo remove <id>",
        "/todo show <id>",
        "/todo clear-done",
    ]
    .join("\n")
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
