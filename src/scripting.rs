use crate::actions::DaemonAction;
use crate::{memory_connection, memory_get_state, memory_set_state};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const SCRIPTING_PLUGIN: &str = "scripting";
const KV_PREFIX: &str = "kv:";

pub fn handle_scripting_command(config: &Value, raw_input: &str) -> Result<Value, String> {
    purge_expired_kv_entries(config)?;
    let message = match parse_kv_command(raw_input)? {
        KvCommand::Help => kv_help_text(),
        KvCommand::Get { key } => get_kv_value(config, &key)?,
        KvCommand::Set {
            key,
            value,
            ttl_seconds,
        } => set_kv_value(config, &key, &value, ttl_seconds)?,
        KvCommand::Unset { key } => unset_kv_value(config, &key)?,
    };
    Ok(DaemonAction::Message {
        plugin: SCRIPTING_PLUGIN.into(),
        message,
    }
    .into_value())
}

enum KvCommand {
    Help,
    Get {
        key: String,
    },
    Set {
        key: String,
        value: String,
        ttl_seconds: Option<i64>,
    },
    Unset {
        key: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KvEntry {
    value: String,
    created_at: i64,
    expires_at: Option<i64>,
}

fn parse_kv_command(raw_input: &str) -> Result<KvCommand, String> {
    let trimmed = raw_input.trim();
    let rest = trimmed
        .strip_prefix("/kv")
        .ok_or_else(|| "scripting kv commands must start with /kv".to_string())?
        .trim();
    if rest.is_empty() {
        return Ok(KvCommand::Help);
    }
    let (subcommand, remainder) = if let Some(index) = rest.find(char::is_whitespace) {
        (&rest[..index], rest[index..].trim())
    } else {
        (rest, "")
    };
    match subcommand {
        "help" => Ok(KvCommand::Help),
        "get" => {
            if remainder.is_empty() {
                Err("Usage: /kv get <key>".into())
            } else {
                Ok(KvCommand::Get {
                    key: remainder.to_string(),
                })
            }
        }
        "set" => {
            let (key, value, ttl_seconds) = parse_kv_set_arguments(remainder)?;
            Ok(KvCommand::Set {
                key,
                value,
                ttl_seconds,
            })
        }
        "unset" => {
            if remainder.is_empty() {
                Err("Usage: /kv unset <key>".into())
            } else {
                Ok(KvCommand::Unset {
                    key: remainder.to_string(),
                })
            }
        }
        _ => Err(format!(
            "Unknown kv command: {}.\n{}",
            subcommand,
            kv_help_text()
        )),
    }
}

fn parse_kv_set_arguments(rest: &str) -> Result<(String, String, Option<i64>), String> {
    if rest.is_empty() {
        return Err("Usage: /kv set <key> <value> [--ttl <duration>]".into());
    }
    let trimmed = rest.trim();
    let (body, ttl_seconds) = strip_trailing_ttl(trimmed)?;
    let Some(index) = body.find(char::is_whitespace) else {
        return Err("Usage: /kv set <key> <value> [--ttl <duration>]".into());
    };
    let key = body[..index].trim();
    let value = body[index..].trim();
    if key.is_empty() || value.is_empty() {
        return Err("Usage: /kv set <key> <value> [--ttl <duration>]".into());
    }
    Ok((key.to_string(), value.to_string(), ttl_seconds))
}

fn strip_trailing_ttl(input: &str) -> Result<(&str, Option<i64>), String> {
    let tokens = token_spans(input);
    if let Some(last) = tokens.last() {
        if let Some(raw_duration) = last.text.strip_prefix("--ttl=") {
            let ttl_seconds = parse_ttl_seconds(raw_duration)?;
            let body = input[..last.start].trim_end();
            if body.is_empty() {
                return Err("Usage: /kv set <key> <value> [--ttl <duration>]".into());
            }
            return Ok((body, Some(ttl_seconds)));
        }
    }
    if tokens.len() >= 2 && tokens[tokens.len() - 2].text == "--ttl" {
        let ttl_seconds = parse_ttl_seconds(tokens.last().expect("ttl token exists").text)?;
        let body = input[..tokens[tokens.len() - 2].start].trim_end();
        if body.is_empty() {
            return Err("Usage: /kv set <key> <value> [--ttl <duration>]".into());
        }
        return Ok((body, Some(ttl_seconds)));
    }
    Ok((input, None))
}

struct TokenSpan<'a> {
    start: usize,
    text: &'a str,
}

fn token_spans(input: &str) -> Vec<TokenSpan<'_>> {
    let mut spans = Vec::new();
    let mut current_start: Option<usize> = None;
    for (index, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = current_start.take() {
                spans.push(TokenSpan {
                    start,
                    text: &input[start..index],
                });
            }
        } else if current_start.is_none() {
            current_start = Some(index);
        }
    }
    if let Some(start) = current_start {
        spans.push(TokenSpan {
            start,
            text: &input[start..],
        });
    }
    spans
}

fn parse_ttl_seconds(raw: &str) -> Result<i64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("TTL duration cannot be empty.".into());
    }
    let (number, multiplier) = match trimmed.chars().last() {
        Some('s') => (&trimmed[..trimmed.len() - 1], 1_i64),
        Some('m') => (&trimmed[..trimmed.len() - 1], 60_i64),
        Some('h') => (&trimmed[..trimmed.len() - 1], 60_i64 * 60),
        Some('d') => (&trimmed[..trimmed.len() - 1], 60_i64 * 60 * 24),
        _ => (trimmed, 1_i64),
    };
    let seconds = number
        .parse::<i64>()
        .map_err(|_| format!("Invalid TTL duration: {trimmed}"))?;
    if seconds <= 0 {
        return Err("TTL duration must be greater than zero.".into());
    }
    seconds
        .checked_mul(multiplier)
        .ok_or_else(|| format!("TTL duration is too large: {trimmed}"))
}

fn get_kv_value(config: &Value, key: &str) -> Result<String, String> {
    match load_kv_entry(config, key)? {
        Some(entry) => Ok(entry.value),
        None => Ok(format!("KV key not found: {}", key)),
    }
}

fn set_kv_value(
    config: &Value,
    key: &str,
    value: &str,
    ttl_seconds: Option<i64>,
) -> Result<String, String> {
    let now = unix_now();
    let expires_at = ttl_seconds.map(|ttl| now + ttl);
    let entry = KvEntry {
        value: value.to_string(),
        created_at: now,
        expires_at,
    };
    memory_set_state(
        config,
        SCRIPTING_PLUGIN,
        &kv_state_key(key),
        &serde_json::to_value(&entry).map_err(|err| err.to_string())?,
    )?;
    let ttl_suffix = ttl_seconds
        .map(|ttl| format!(" TTL {}", format_ttl_seconds(ttl)))
        .unwrap_or_default();
    Ok(format!("Set kv key {}.{}", key, ttl_suffix))
}

fn unset_kv_value(config: &Value, key: &str) -> Result<String, String> {
    let conn = memory_connection(config)?;
    let deleted = conn
        .execute(
            "DELETE FROM state WHERE plugin = ?1 AND key = ?2",
            params![SCRIPTING_PLUGIN, kv_state_key(key)],
        )
        .map_err(|err| err.to_string())?;
    if deleted > 0 {
        Ok(format!("Removed kv key {}.", key))
    } else {
        Ok(format!("KV key was not set: {}", key))
    }
}

fn load_kv_entry(config: &Value, key: &str) -> Result<Option<KvEntry>, String> {
    let Some(value) = memory_get_state(config, SCRIPTING_PLUGIN, &kv_state_key(key))? else {
        return Ok(None);
    };
    let entry: KvEntry = serde_json::from_value(value).map_err(|err| err.to_string())?;
    if entry
        .expires_at
        .map(|expires_at| expires_at <= unix_now())
        .unwrap_or(false)
    {
        let _ = unset_kv_value(config, key)?;
        return Ok(None);
    }
    Ok(Some(entry))
}

fn purge_expired_kv_entries(config: &Value) -> Result<usize, String> {
    let conn = memory_connection(config)?;
    let mut stmt = conn
        .prepare(
            "SELECT key, value_json
             FROM state
             WHERE plugin = ?1 AND key LIKE ?2",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![SCRIPTING_PLUGIN, format!("{KV_PREFIX}%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| err.to_string())?;
    let now = unix_now();
    let mut expired_keys = Vec::new();
    for row in rows {
        let (key, raw_value) = row.map_err(|err| err.to_string())?;
        let Ok(entry) = serde_json::from_str::<KvEntry>(&raw_value) else {
            continue;
        };
        if entry
            .expires_at
            .map(|expires_at| expires_at <= now)
            .unwrap_or(false)
        {
            expired_keys.push(key);
        }
    }
    for key in &expired_keys {
        conn.execute(
            "DELETE FROM state WHERE plugin = ?1 AND key = ?2",
            params![SCRIPTING_PLUGIN, key],
        )
        .map_err(|err| err.to_string())?;
    }
    Ok(expired_keys.len())
}

fn kv_help_text() -> String {
    [
        "Scripting commands:",
        "/kv help",
        "/kv get <key>",
        "/kv set <key> <value> [--ttl <duration>]",
        "/kv unset <key>",
        "TTL durations accept seconds by default or s/m/h/d suffixes.",
    ]
    .join("\n")
}

fn kv_state_key(key: &str) -> String {
    format!("{KV_PREFIX}{key}")
}

fn format_ttl_seconds(ttl_seconds: i64) -> String {
    if ttl_seconds % (60 * 60 * 24) == 0 {
        format!("{}d", ttl_seconds / (60 * 60 * 24))
    } else if ttl_seconds % (60 * 60) == 0 {
        format!("{}h", ttl_seconds / (60 * 60))
    } else if ttl_seconds % 60 == 0 {
        format!("{}m", ttl_seconds / 60)
    } else {
        format!("{}s", ttl_seconds)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::{parse_kv_set_arguments, parse_ttl_seconds};

    #[test]
    fn set_arguments_parse_trailing_ttl() {
        let (key, value, ttl) = parse_kv_set_arguments("session-token abc123 --ttl 5m").unwrap();
        assert_eq!(key, "session-token");
        assert_eq!(value, "abc123");
        assert_eq!(ttl, Some(300));
    }

    #[test]
    fn set_arguments_allow_spaces_in_value() {
        let (key, value, ttl) =
            parse_kv_set_arguments("greeting hello there general kenobi").unwrap();
        assert_eq!(key, "greeting");
        assert_eq!(value, "hello there general kenobi");
        assert_eq!(ttl, None);
    }

    #[test]
    fn ttl_parser_accepts_suffixes() {
        assert_eq!(parse_ttl_seconds("30").unwrap(), 30);
        assert_eq!(parse_ttl_seconds("30s").unwrap(), 30);
        assert_eq!(parse_ttl_seconds("5m").unwrap(), 300);
        assert_eq!(parse_ttl_seconds("2h").unwrap(), 7200);
        assert_eq!(parse_ttl_seconds("1d").unwrap(), 86400);
    }
}
