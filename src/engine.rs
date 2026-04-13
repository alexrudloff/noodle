use crate::actions::DaemonAction;
use crate::executor;
use crate::tooling::{ToolDefinition, tools_for_plugin};
use serde_json::Value;

pub use crate::executor::ChatExecutionConfig;

pub fn run_chat_execution(
    config: &Value,
    request: ChatExecutionConfig,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    executor::run_chat_execution(config, request, streaming, emitter, model_output)
}

pub fn resume_chat_execution_from_permission(
    config: &Value,
    permission_id: &str,
    decision: &str,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    executor::resume_chat_execution_from_permission(
        config,
        permission_id,
        decision,
        streaming,
        emitter,
        model_output,
    )
}

pub fn resume_task_execution(
    config: &Value,
    task_id: &str,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    executor::resume_task_execution(config, task_id, streaming, emitter, model_output)
}

pub fn plugin_tool_calling_enabled(config: &Value, plugin_id: &str) -> bool {
    let pointer = format!("/plugins/{plugin_id}/tool_calling");
    match config.pointer(&pointer) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or(0) != 0,
        Some(Value::String(value)) => value == "1" || value.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

pub fn plugin_task_execution_enabled(config: &Value, plugin_id: &str) -> bool {
    let pointer = format!("/plugins/{plugin_id}/task_execution");
    match config.pointer(&pointer) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or(0) != 0,
        Some(Value::String(value)) => value == "1" || value.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

pub fn plugin_max_tool_rounds(config: &Value, plugin_id: &str) -> usize {
    let pointer = format!("/plugins/{plugin_id}/max_tool_rounds");
    match config.pointer(&pointer) {
        Some(Value::Number(value)) => value.as_u64().unwrap_or(8) as usize,
        Some(Value::String(value)) => value.parse::<usize>().unwrap_or(8),
        _ => 8,
    }
    .max(1)
}

pub fn plugin_max_replans(config: &Value, plugin_id: &str) -> usize {
    let pointer = format!("/plugins/{plugin_id}/max_replans");
    match config.pointer(&pointer) {
        Some(Value::Number(value)) => value.as_u64().unwrap_or(1) as usize,
        Some(Value::String(value)) => value.parse::<usize>().unwrap_or(1),
        _ => 1,
    }
}

pub fn plugin_tools_for_config(config: &Value, plugin_id: &str) -> Vec<ToolDefinition> {
    tools_for_plugin(config, plugin_id)
}
