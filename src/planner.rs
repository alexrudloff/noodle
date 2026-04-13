use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    pub tool: String,
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub summary: String,
    pub steps: Vec<TaskStep>,
}

#[derive(Debug, Clone)]
pub enum PlannedChatStep {
    Final(String),
    Tool(TaskStep),
    Plan(TaskPlan),
}

pub fn parse_planned_chat_step(raw: &str) -> PlannedChatStep {
    let cleaned = clean_text(raw);

    if let Some(plan) = parse_plan_block(&cleaned) {
        return PlannedChatStep::Plan(plan);
    }

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("FINAL:") {
            return PlannedChatStep::Final(rest.trim().to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("TOOL:") {
            let rest = rest.trim();
            if let Some((name, args)) = split_tool_invocation(rest) {
                return PlannedChatStep::Tool(TaskStep { tool: name, args });
            }
        }
        if let Some(rest) = trimmed.strip_prefix("STEP:") {
            let rest = rest.trim();
            if let Some((name, args)) = split_tool_invocation(rest) {
                return PlannedChatStep::Tool(TaskStep { tool: name, args });
            }
        }
    }

    PlannedChatStep::Final(cleaned)
}

fn parse_plan_block(text: &str) -> Option<TaskPlan> {
    let mut in_plan = false;
    let mut summary = String::new();
    let mut steps = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("PLAN:") {
            in_plan = true;
            summary = rest.trim().to_string();
            continue;
        }
        if !in_plan {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("STEP:") {
            let rest = rest.trim();
            if let Some((tool, args)) = split_tool_invocation(rest) {
                steps.push(TaskStep { tool, args });
            }
        } else if trimmed.starts_with("FINAL:") || trimmed.starts_with("TOOL:") {
            break;
        }
    }

    if in_plan && !steps.is_empty() {
        Some(TaskPlan { summary, steps })
    } else {
        None
    }
}

fn split_tool_invocation(text: &str) -> Option<(String, Value)> {
    let space = text.find(' ')?;
    let name = text[..space].trim().to_string();
    let raw_json = text[space..].trim();
    let args = serde_json::from_str::<Value>(raw_json).ok()?;
    Some((name, args))
}

fn clean_text(text: &str) -> String {
    let mut cleaned = text.trim().replace("```json", "").replace("```", "");
    for pattern in ["<think>", "</think>", "<thinking>", "</thinking>"] {
        cleaned = cleaned.replace(pattern, "");
    }
    cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{PlannedChatStep, parse_planned_chat_step};

    #[test]
    fn standalone_step_is_treated_as_a_tool_call() {
        match parse_planned_chat_step(r#"STEP: file_read {"path":"README.md"}"#) {
            PlannedChatStep::Tool(step) => {
                assert_eq!(step.tool, "file_read");
                assert_eq!(step.args["path"], "README.md");
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }

    #[test]
    fn interactive_shell_tool_call_is_parsed() {
        match parse_planned_chat_step(
            r#"TOOL: interactive_shell_start {"command":"printf 'name? '; read name; printf 'hi:%s' $name"}"#,
        ) {
            PlannedChatStep::Tool(step) => {
                assert_eq!(step.tool, "interactive_shell_start");
                assert_eq!(
                    step.args["command"].as_str(),
                    Some("printf 'name? '; read name; printf 'hi:%s' $name")
                );
            }
            other => panic!("expected interactive shell tool call, got {other:?}"),
        }
    }
}
