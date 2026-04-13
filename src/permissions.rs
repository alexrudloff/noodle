use crate::planner::TaskStep;
use crate::tasks::TaskRecord;
use crate::tooling::{active_artifact_content, deactivate_artifact, upsert_memory_artifact};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatExecutionSnapshot {
    pub plugin: String,
    pub input: String,
    pub working_directory: String,
    pub base_prompt: String,
    pub memory_context: String,
    pub include_tool_context: bool,
    pub tool_calling_enabled: bool,
    pub task_execution_enabled: bool,
    pub max_tool_rounds: usize,
    pub max_replans: usize,
    pub available_tool_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTurnSnapshot {
    pub tool: String,
    pub args: Value,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PendingContinuation {
    ToolLoop {
        remaining_rounds: usize,
    },
    Plan {
        task: TaskRecord,
        current_step_index: usize,
        remaining_steps: Vec<TaskStep>,
        replans_remaining: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPermissionRequest {
    pub id: String,
    pub plugin: String,
    pub tool: String,
    pub permission_class: String,
    pub summary: String,
    pub request: ChatExecutionSnapshot,
    pub tool_turns: Vec<ToolTurnSnapshot>,
    pub pending_step: TaskStep,
    pub continuation: PendingContinuation,
}

pub fn persist_pending_permission(
    config: &Value,
    pending: &PendingPermissionRequest,
) -> Result<(), String> {
    let kind = pending_kind(&pending.id);
    upsert_memory_artifact(
        config,
        "permissions",
        &kind,
        &serde_json::to_string(pending).map_err(|err| err.to_string())?,
        &json!({
            "plugin": pending.plugin,
            "tool": pending.tool,
            "permission_class": pending.permission_class,
            "status": "pending",
        }),
    )
}

pub fn load_pending_permission(
    config: &Value,
    id: &str,
) -> Result<Option<PendingPermissionRequest>, String> {
    let kind = pending_kind(id);
    let Some(content) = active_artifact_content(config, "permissions", &kind)? else {
        return Ok(None);
    };
    let parsed = serde_json::from_str::<PendingPermissionRequest>(&content)
        .map_err(|err| err.to_string())?;
    Ok(Some(parsed))
}

pub fn clear_pending_permission(config: &Value, id: &str) -> Result<(), String> {
    deactivate_artifact(config, "permissions", &pending_kind(id))
}

fn pending_kind(id: &str) -> String {
    format!("pending:{id}")
}
