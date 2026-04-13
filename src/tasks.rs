use crate::planner::TaskPlan;
use crate::tooling::{ToolCallResult, upsert_memory_artifact};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub plugin: String,
    pub goal: String,
    pub summary: String,
    pub status: String,
    pub steps: Vec<TaskStepRecord>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStepRecord {
    pub index: usize,
    pub tool: String,
    pub args: Value,
    pub status: String,
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRuntimeState {
    pub task: TaskRecord,
    pub request: Value,
    pub current_step_index: usize,
    pub remaining_steps: Vec<crate::planner::TaskStep>,
    pub tool_turns: Vec<Value>,
    pub replans_remaining: usize,
    pub status: String,
}

impl TaskRecord {
    pub fn from_plan(plugin: &str, goal: &str, plan: &TaskPlan) -> Self {
        let now = unix_now();
        Self {
            id: format!("task-{}-{}", plugin, now),
            plugin: plugin.to_string(),
            goal: goal.to_string(),
            summary: if plan.summary.trim().is_empty() {
                goal.to_string()
            } else {
                plan.summary.clone()
            },
            status: "planned".into(),
            steps: plan
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| TaskStepRecord {
                    index,
                    tool: step.tool.clone(),
                    args: step.args.clone(),
                    status: "pending".into(),
                    result: None,
                })
                .collect(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn mark_running(&mut self, index: usize) {
        self.status = "running".into();
        self.updated_at = unix_now();
        if let Some(step) = self.steps.get_mut(index) {
            step.status = "running".into();
        }
    }

    pub fn mark_step_finished(&mut self, index: usize, result: &ToolCallResult) {
        self.updated_at = unix_now();
        if let Some(step) = self.steps.get_mut(index) {
            step.status = "done".into();
            step.result = Some(result.output.clone());
        }
    }

    pub fn mark_step_failed(&mut self, index: usize, error: &str) {
        self.status = "running".into();
        self.updated_at = unix_now();
        if let Some(step) = self.steps.get_mut(index) {
            step.status = "failed".into();
            step.result = Some(json!({ "error": error }));
        }
    }

    pub fn replace_remaining_steps(
        &mut self,
        start_index: usize,
        steps: &[crate::planner::TaskStep],
    ) {
        self.updated_at = unix_now();
        self.steps.truncate(start_index);
        self.steps.extend(
            steps
                .iter()
                .enumerate()
                .map(|(offset, step)| TaskStepRecord {
                    index: start_index + offset,
                    tool: step.tool.clone(),
                    args: step.args.clone(),
                    status: "pending".into(),
                    result: None,
                }),
        );
    }

    pub fn mark_completed(&mut self) {
        self.status = "completed".into();
        self.updated_at = unix_now();
    }

    pub fn mark_failed(&mut self, reason: &str) {
        self.status = "failed".into();
        self.updated_at = unix_now();
        if let Some(step) = self.steps.iter_mut().find(|step| step.status == "running") {
            step.status = "failed".into();
            step.result = Some(json!({ "error": reason }));
        }
    }
}

pub fn persist_task_record(config: &Value, task: &TaskRecord) -> Result<(), String> {
    upsert_memory_artifact(
        config,
        "tasks",
        &task.id,
        &serde_json::to_string_pretty(task).map_err(|err| err.to_string())?,
        &json!({
            "plugin": task.plugin,
            "goal": task.goal,
            "status": task.status,
            "kind": "task_record",
        }),
    )
}

pub fn persist_task_runtime_state(config: &Value, state: &TaskRuntimeState) -> Result<(), String> {
    upsert_memory_artifact(
        config,
        "tasks",
        &runtime_kind(&state.task.id),
        &serde_json::to_string_pretty(state).map_err(|err| err.to_string())?,
        &json!({
            "plugin": state.task.plugin,
            "task_id": state.task.id,
            "status": state.status,
            "kind": "task_runtime",
        }),
    )
}

pub fn load_task_runtime_state(
    config: &Value,
    task_id: &str,
) -> Result<Option<TaskRuntimeState>, String> {
    let conn = memory_connection(config)?;
    conn.query_row(
        "SELECT content
         FROM artifacts
         WHERE plugin = 'tasks' AND kind = ?1 AND active = 1
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![runtime_kind(task_id)],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|err| err.to_string())?
    .map(|content| {
        serde_json::from_str::<TaskRuntimeState>(&content).map_err(|err| err.to_string())
    })
    .transpose()
}

pub fn clear_task_runtime_state(config: &Value, task_id: &str) -> Result<(), String> {
    let conn = memory_connection(config)?;
    conn.execute(
        "UPDATE artifacts SET active = 0 WHERE plugin = 'tasks' AND kind = ?1 AND active = 1",
        params![runtime_kind(task_id)],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn load_task_record(config: &Value, task_id: &str) -> Result<Option<TaskRecord>, String> {
    let conn = memory_connection(config)?;
    conn.query_row(
        "SELECT content
         FROM artifacts
         WHERE plugin = 'tasks' AND kind = ?1 AND active = 1
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![task_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|err| err.to_string())?
    .map(|content| serde_json::from_str::<TaskRecord>(&content).map_err(|err| err.to_string()))
    .transpose()
}

pub fn list_task_records(
    config: &Value,
    limit: usize,
    status_filter: Option<&str>,
) -> Result<Vec<TaskRecord>, String> {
    let conn = memory_connection(config)?;
    let mut stmt = conn
        .prepare(
            "SELECT content
             FROM artifacts
             WHERE plugin = 'tasks'
               AND kind LIKE 'task-%'
               AND active = 1
             ORDER BY updated_at DESC, id DESC
             LIMIT ?1",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![limit as i64], |row| row.get::<_, String>(0))
        .map_err(|err| err.to_string())?;
    let mut tasks = Vec::new();
    for row in rows {
        let content = row.map_err(|err| err.to_string())?;
        let task = serde_json::from_str::<TaskRecord>(&content).map_err(|err| err.to_string())?;
        if let Some(status) = status_filter {
            if task.status != status {
                continue;
            }
        }
        tasks.push(task);
    }
    Ok(tasks)
}

pub fn cancel_task(config: &Value, task_id: &str) -> Result<Option<TaskRecord>, String> {
    let Some(mut task) = load_task_record(config, task_id)? else {
        return Ok(None);
    };
    task.status = "cancelled".into();
    task.updated_at = unix_now();
    persist_task_record(config, &task)?;
    clear_task_runtime_state(config, task_id)?;
    Ok(Some(task))
}

fn runtime_kind(task_id: &str) -> String {
    format!("runtime:{task_id}")
}

fn memory_connection(config: &Value) -> Result<Connection, String> {
    let path = memory_path(config);
    let resolved = expand_home(&path);
    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let conn = Connection::open(resolved).map_err(|err| err.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plugin TEXT NOT NULL,
            kind TEXT NOT NULL,
            key TEXT NOT NULL,
            value_json TEXT NOT NULL,
            created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS state (
            plugin TEXT NOT NULL,
            key TEXT NOT NULL,
            value_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (plugin, key)
         );
         CREATE TABLE IF NOT EXISTS artifacts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plugin TEXT NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            source_json TEXT NOT NULL,
            active INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
         );",
    )
    .map_err(|err| err.to_string())?;
    Ok(conn)
}

fn memory_path(config: &Value) -> String {
    match lookup(config, "memory.path") {
        Some(Value::String(path)) => path.clone(),
        _ => default_memory_path(),
    }
}

fn default_memory_path() -> String {
    env::var("NOODLE_MEMORY_DB").unwrap_or_else(|_| "~/.noodle/memory.db".into())
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

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
