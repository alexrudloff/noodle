use crate::actions::DaemonAction;
use crate::{expand_home, memory_connection};
use rusqlite::params;
use serde_json::Value;
use std::fmt::Write as _;

const MEMORY_PLUGIN: &str = "memory";

pub fn handle_memory_command(config: &Value, raw_input: &str) -> Result<Value, String> {
    let message = match parse_memory_command(raw_input)? {
        MemoryCommand::Help => memory_help_text(),
        MemoryCommand::Summary => render_memory_summary(config)?,
        MemoryCommand::Search { term } => render_memory_search(config, &term)?,
        MemoryCommand::Clear { scope } => clear_memory_scope(config, &scope)?,
    };
    Ok(DaemonAction::Message {
        plugin: MEMORY_PLUGIN.into(),
        message,
    }
    .into_value())
}

enum MemoryCommand {
    Help,
    Summary,
    Search { term: String },
    Clear { scope: String },
}

fn parse_memory_command(raw_input: &str) -> Result<MemoryCommand, String> {
    let trimmed = raw_input.trim();
    let rest = trimmed
        .strip_prefix("/memory")
        .ok_or_else(|| "memory commands must start with /memory".to_string())?
        .trim();
    if rest.is_empty() {
        return Ok(MemoryCommand::Summary);
    }
    let (subcommand, remainder) = if let Some(index) = rest.find(char::is_whitespace) {
        (&rest[..index], rest[index..].trim())
    } else {
        (rest, "")
    };
    match subcommand {
        "help" => Ok(MemoryCommand::Help),
        "search" => {
            if remainder.is_empty() {
                Err("Usage: /memory search <term>".into())
            } else {
                Ok(MemoryCommand::Search {
                    term: remainder.to_string(),
                })
            }
        }
        "clear" => {
            if remainder.is_empty() {
                Err("Usage: /memory clear <plugin|all>".into())
            } else {
                Ok(MemoryCommand::Clear {
                    scope: remainder.to_string(),
                })
            }
        }
        _ => Err(format!(
            "Unknown memory command: {}.\n{}",
            subcommand,
            memory_help_text()
        )),
    }
}

fn render_memory_summary(config: &Value) -> Result<String, String> {
    let conn = memory_connection(config)?;
    let events: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .map_err(|err| err.to_string())?;
    let state: i64 = conn
        .query_row("SELECT COUNT(*) FROM state", [], |row| row.get(0))
        .map_err(|err| err.to_string())?;
    let artifacts_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM artifacts", [], |row| row.get(0))
        .map_err(|err| err.to_string())?;
    let artifacts_active: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM artifacts WHERE active = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|err| err.to_string())?;

    let mut plugin_lines = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT plugin,
                    (SELECT COUNT(*) FROM events e WHERE e.plugin = p.plugin) AS event_count,
                    (SELECT COUNT(*) FROM state s WHERE s.plugin = p.plugin) AS state_count,
                    (SELECT COUNT(*) FROM artifacts a WHERE a.plugin = p.plugin AND a.active = 1) AS active_artifacts
             FROM (
               SELECT plugin FROM events
               UNION
               SELECT plugin FROM state
               UNION
               SELECT plugin FROM artifacts
             ) p
             ORDER BY plugin",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|err| err.to_string())?;
    for row in rows {
        let (plugin, event_count, state_count, active_artifacts) =
            row.map_err(|err| err.to_string())?;
        plugin_lines.push(format!(
            "- {}: {} events, {} state keys, {} active artifacts",
            plugin, event_count, state_count, active_artifacts
        ));
    }

    let memory_path = expand_home(
        config
            .get("memory")
            .and_then(|value| value.get("path"))
            .and_then(Value::as_str)
            .unwrap_or("~/.noodle/memory.db"),
    );

    let mut output = String::new();
    writeln!(&mut output, "Memory DB: {}", memory_path.display()).ok();
    writeln!(&mut output, "Events: {}", events).ok();
    writeln!(&mut output, "State keys: {}", state).ok();
    writeln!(
        &mut output,
        "Artifacts: {} active / {} total",
        artifacts_active, artifacts_total
    )
    .ok();
    if !plugin_lines.is_empty() {
        writeln!(&mut output, "Plugins:").ok();
        for line in plugin_lines {
            writeln!(&mut output, "{}", line).ok();
        }
    }
    Ok(output.trim_end().to_string())
}

fn render_memory_search(config: &Value, term: &str) -> Result<String, String> {
    let conn = memory_connection(config)?;
    let pattern = format!("%{}%", term.to_lowercase());
    let mut lines = Vec::new();

    let mut event_stmt = conn
        .prepare(
            "SELECT plugin, kind, key, value_json
             FROM events
             WHERE lower(key) LIKE ?1 OR lower(value_json) LIKE ?1
             ORDER BY created_at DESC, id DESC
             LIMIT 5",
        )
        .map_err(|err| err.to_string())?;
    let event_rows = event_stmt
        .query_map(params![pattern.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|err| err.to_string())?;
    for row in event_rows {
        let (plugin, kind, key, value_json) = row.map_err(|err| err.to_string())?;
        lines.push(format!(
            "[event] {}.{} {}",
            plugin,
            kind,
            render_search_entry(&key, &value_json)
        ));
    }

    let mut state_stmt = conn
        .prepare(
            "SELECT plugin, key, value_json
             FROM state
             WHERE lower(key) LIKE ?1 OR lower(value_json) LIKE ?1
             ORDER BY updated_at DESC
             LIMIT 5",
        )
        .map_err(|err| err.to_string())?;
    let state_rows = state_stmt
        .query_map(params![pattern.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| err.to_string())?;
    for row in state_rows {
        let (plugin, key, value_json) = row.map_err(|err| err.to_string())?;
        lines.push(format!(
            "[state] {}.{} {}",
            plugin,
            key,
            render_search_entry(&key, &value_json)
        ));
    }

    let mut artifact_stmt = conn
        .prepare(
            "SELECT plugin, kind, content
             FROM artifacts
             WHERE lower(kind) LIKE ?1 OR lower(content) LIKE ?1 OR lower(source_json) LIKE ?1
             ORDER BY updated_at DESC, id DESC
             LIMIT 5",
        )
        .map_err(|err| err.to_string())?;
    let artifact_rows = artifact_stmt
        .query_map(params![pattern.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| err.to_string())?;
    for row in artifact_rows {
        let (plugin, kind, content) = row.map_err(|err| err.to_string())?;
        lines.push(format!(
            "[artifact] {}.{} {}",
            plugin,
            kind,
            trim_one_line(&content, 120)
        ));
    }

    if lines.is_empty() {
        return Ok(format!("No memory matches for \"{}\".", term));
    }

    Ok(format!("Memory search: {}\n{}", term, lines.join("\n")))
}

fn clear_memory_scope(config: &Value, scope: &str) -> Result<String, String> {
    let conn = memory_connection(config)?;
    let (events_deleted, state_deleted, artifacts_deleted) = if scope == "all" {
        let events = conn
            .execute("DELETE FROM events", [])
            .map_err(|err| err.to_string())?;
        let state = conn
            .execute("DELETE FROM state", [])
            .map_err(|err| err.to_string())?;
        let artifacts = conn
            .execute("DELETE FROM artifacts", [])
            .map_err(|err| err.to_string())?;
        (events, state, artifacts)
    } else {
        let events = conn
            .execute("DELETE FROM events WHERE plugin = ?1", params![scope])
            .map_err(|err| err.to_string())?;
        let state = conn
            .execute("DELETE FROM state WHERE plugin = ?1", params![scope])
            .map_err(|err| err.to_string())?;
        let artifacts = conn
            .execute("DELETE FROM artifacts WHERE plugin = ?1", params![scope])
            .map_err(|err| err.to_string())?;
        (events, state, artifacts)
    };

    Ok(format!(
        "Cleared memory for {}: {} events, {} state keys, {} artifacts.",
        scope, events_deleted, state_deleted, artifacts_deleted
    ))
}

fn render_search_entry(key: &str, value: &str) -> String {
    if key.trim().is_empty() {
        trim_one_line(value, 120)
    } else {
        format!("{} {}", key, trim_one_line(value, 100))
    }
}

fn trim_one_line(text: &str, limit: usize) -> String {
    let flattened = text.replace('\n', " ");
    if flattened.len() <= limit {
        flattened
    } else {
        format!("{}...", &flattened[..limit])
    }
}

fn memory_help_text() -> String {
    [
        "Memory commands:",
        "/memory",
        "/memory help",
        "/memory search <term>",
        "/memory clear <plugin|all>",
    ]
    .join("\n")
}
