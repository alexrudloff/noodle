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
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let action = value
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing action".to_string())?;
        let plugin = value
            .get("plugin")
            .and_then(Value::as_str)
            .unwrap_or("host")
            .to_string();
        let required_string = |key: &str| -> Result<String, String> {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("missing {key}"))
        };
        let required_usize = |key: &str| -> Result<usize, String> {
            value
                .get(key)
                .and_then(Value::as_u64)
                .map(|number| number as usize)
                .ok_or_else(|| format!("missing {key}"))
        };

        match action {
            "message" => Ok(Self::Message {
                plugin,
                message: required_string("message")?,
            }),
            "reload_runtime" => Ok(Self::ReloadRuntime {
                plugin,
                message: required_string("message")?,
            }),
            "ask" => Ok(Self::Ask {
                plugin,
                question: required_string("question")?,
            }),
            "run" => Ok(Self::Run {
                plugin,
                command: required_string("command")?,
                explanation: required_string("explanation")?,
            }),
            "select" => Ok(Self::Select {
                plugin,
                choices: value
                    .get("choices")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "missing choices".to_string())?
                    .iter()
                    .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                    .collect(),
            }),
            "permission_request" => Ok(Self::PermissionRequest {
                plugin,
                permission_id: required_string("permission_id")?,
                tool: required_string("tool")?,
                permission_class: required_string("permission_class")?,
                summary: required_string("summary")?,
            }),
            "tool_step" => Ok(Self::ToolStep {
                plugin,
                tool: required_string("tool")?,
                status: required_string("status")?,
                summary: required_string("summary")?,
            }),
            "session_started" => Ok(Self::SessionStarted {
                plugin,
                command: required_string("command")?,
            }),
            "session_input" => Ok(Self::SessionInput {
                plugin,
                text: required_string("text")?,
            }),
            "session_output" => Ok(Self::SessionOutput {
                plugin,
                text: required_string("text")?,
            }),
            "session_closed" => Ok(Self::SessionClosed { plugin }),
            "task_started" => Ok(Self::TaskStarted {
                plugin,
                task_id: required_string("task_id")?,
                summary: required_string("summary")?,
            }),
            "task_step" => Ok(Self::TaskStep {
                plugin,
                task_id: required_string("task_id")?,
                index: required_usize("index")?,
                total: required_usize("total")?,
                tool: required_string("tool")?,
                status: required_string("status")?,
                summary: required_string("summary")?,
            }),
            "task_finished" => Ok(Self::TaskFinished {
                plugin,
                task_id: required_string("task_id")?,
                status: required_string("status")?,
                summary: required_string("summary")?,
            }),
            "batch" => Ok(Self::Batch {
                plugin,
                actions: value
                    .get("items")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "missing items".to_string())?
                    .iter()
                    .map(Self::from_value)
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            "noop" => Ok(Self::Noop { plugin }),
            other => Err(format!("unknown action: {other}")),
        }
    }

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
