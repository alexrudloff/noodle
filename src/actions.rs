use serde_json::{Value, json};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum DaemonAction {
    Message {
        plugin: String,
        message: String,
    },
    ReloadRuntime {
        plugin: String,
        message: String,
    },
    Ask {
        plugin: String,
        question: String,
    },
    Run {
        plugin: String,
        command: String,
        explanation: String,
    },
    Select {
        plugin: String,
        choices: Vec<String>,
    },
    PermissionRequest {
        plugin: String,
        permission_id: String,
        tool: String,
        permission_class: String,
        summary: String,
    },
    ToolStep {
        plugin: String,
        tool: String,
        status: String,
        summary: String,
    },
    SessionStarted {
        plugin: String,
        command: String,
    },
    SessionInput {
        plugin: String,
        text: String,
    },
    SessionOutput {
        plugin: String,
        text: String,
    },
    SessionClosed {
        plugin: String,
    },
    TaskStarted {
        plugin: String,
        task_id: String,
        summary: String,
    },
    TaskStep {
        plugin: String,
        task_id: String,
        index: usize,
        total: usize,
        tool: String,
        status: String,
        summary: String,
    },
    TaskFinished {
        plugin: String,
        task_id: String,
        status: String,
        summary: String,
    },
    Batch {
        plugin: String,
        actions: Vec<DaemonAction>,
    },
    Noop {
        plugin: String,
    },
}

impl DaemonAction {
    pub fn into_value(self) -> Value {
        match self {
            Self::Message { plugin, message } => {
                json!({"plugin": plugin, "action": "message", "message": message})
            }
            Self::ReloadRuntime { plugin, message } => {
                json!({"plugin": plugin, "action": "reload_runtime", "message": message})
            }
            Self::Ask { plugin, question } => {
                json!({"plugin": plugin, "action": "ask", "question": question})
            }
            Self::Run {
                plugin,
                command,
                explanation,
            } => json!({
                "plugin": plugin,
                "action": "run",
                "command": command,
                "explanation": explanation
            }),
            Self::Select { plugin, choices } => {
                json!({"plugin": plugin, "action": "select", "choices": choices})
            }
            Self::PermissionRequest {
                plugin,
                permission_id,
                tool,
                permission_class,
                summary,
            } => json!({
                "plugin": plugin,
                "action": "permission_request",
                "permission_id": permission_id,
                "tool": tool,
                "permission_class": permission_class,
                "summary": summary,
            }),
            Self::ToolStep {
                plugin,
                tool,
                status,
                summary,
            } => json!({
                "plugin": plugin,
                "action": "tool_step",
                "tool": tool,
                "status": status,
                "summary": summary,
            }),
            Self::SessionStarted { plugin, command } => json!({
                "plugin": plugin,
                "action": "session_started",
                "command": command,
            }),
            Self::SessionInput { plugin, text } => json!({
                "plugin": plugin,
                "action": "session_input",
                "text": text,
            }),
            Self::SessionOutput { plugin, text } => json!({
                "plugin": plugin,
                "action": "session_output",
                "text": text,
            }),
            Self::SessionClosed { plugin } => json!({
                "plugin": plugin,
                "action": "session_closed",
            }),
            Self::TaskStarted {
                plugin,
                task_id,
                summary,
            } => json!({
                "plugin": plugin,
                "action": "task_started",
                "task_id": task_id,
                "summary": summary,
            }),
            Self::TaskStep {
                plugin,
                task_id,
                index,
                total,
                tool,
                status,
                summary,
            } => json!({
                "plugin": plugin,
                "action": "task_step",
                "task_id": task_id,
                "index": index,
                "total": total,
                "tool": tool,
                "status": status,
                "summary": summary,
            }),
            Self::TaskFinished {
                plugin,
                task_id,
                status,
                summary,
            } => json!({
                "plugin": plugin,
                "action": "task_finished",
                "task_id": task_id,
                "status": status,
                "summary": summary,
            }),
            Self::Batch { plugin, actions } => json!({
                "plugin": plugin,
                "action": "batch",
                "items": actions.into_iter().map(DaemonAction::into_value).collect::<Vec<_>>(),
            }),
            Self::Noop { plugin } => {
                json!({"plugin": plugin, "action": "noop"})
            }
        }
    }

    pub fn primary_text(&self) -> Option<String> {
        match self {
            Self::Message { message, .. } => Some(message.clone()),
            Self::ReloadRuntime { message, .. } => Some(message.clone()),
            Self::Ask { question, .. } => Some(question.clone()),
            Self::Run { command, .. } => Some(command.clone()),
            Self::Select { choices, .. } => Some(choices.join("\n")),
            Self::PermissionRequest { summary, .. } => Some(summary.clone()),
            Self::TaskFinished { summary, .. } => Some(summary.clone()),
            Self::Batch { actions, .. } => {
                actions.iter().rev().find_map(DaemonAction::primary_text)
            }
            Self::ToolStep { .. }
            | Self::SessionStarted { .. }
            | Self::SessionInput { .. }
            | Self::SessionOutput { .. }
            | Self::SessionClosed { .. }
            | Self::TaskStarted { .. }
            | Self::TaskStep { .. }
            | Self::Noop { .. } => None,
        }
    }
}
