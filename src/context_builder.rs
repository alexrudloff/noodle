use crate::tooling::ToolDefinition;
use serde_json::Value;

#[derive(Debug, Clone)]
struct PromptSection {
    title: &'static str,
    body: String,
}

#[cfg_attr(not(test), allow(dead_code))]
pub struct EventPromptInput<'a> {
    pub mode: &'a str,
    pub template: &'a str,
    pub input: &'a str,
    pub cwd: &'a str,
    pub shell: &'a str,
    pub exit_status: i64,
    pub recent_command: &'a str,
    pub soul: Option<&'a str>,
    pub extra_sections: &'a [String],
}

#[derive(Debug, Clone)]
pub struct ToolTurnContext {
    pub tool: String,
    pub args: Value,
    pub result: Value,
}

#[derive(Debug, Clone)]
pub enum TaskDirectiveContext {
    Finalize {
        summary: String,
    },
    ContinueInteractive {
        summary: String,
    },
    VerifyDirectAnswer {
        draft_answer: String,
        required_tool_use_reason: Option<String>,
    },
    ForceToolChoice {
        reason: String,
    },
    Replan {
        summary: String,
        goal: String,
        failed_step_tool: String,
        failed_step_args: Value,
        error: String,
    },
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn build_event_prompt(input: EventPromptInput<'_>) -> String {
    let prompt = input
        .template
        .replace("{mode}", input.mode)
        .replace("{cwd}", input.cwd)
        .replace("{shell}", input.shell)
        .replace("{exit_status}", &input.exit_status.to_string())
        .replace("{recent_command}", input.recent_command)
        .replace("{user_input}", input.input);

    let mut sections = Vec::new();
    if let Some(soul) = input.soul.filter(|value| !value.trim().is_empty()) {
        sections.push(PromptSection {
            title: "Identity",
            body: soul.trim().to_string(),
        });
    }
    sections.push(PromptSection {
        title: "Operating Instructions",
        body: prompt.trim().to_string(),
    });
    let extra = join_sections(input.extra_sections);
    if !extra.is_empty() {
        sections.push(PromptSection {
            title: "Additional Context",
            body: extra,
        });
    }
    render_sections(&sections)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn build_chat_base_prompt(
    instructions: &str,
    request: &str,
    cwd: &str,
    shell: &str,
    recent_command: &str,
    soul: Option<&str>,
    extra_sections: &[String],
) -> String {
    let mut sections = Vec::new();
    if let Some(soul) = soul.filter(|value| !value.trim().is_empty()) {
        sections.push(PromptSection {
            title: "Identity",
            body: soul.trim().to_string(),
        });
    }

    let instructions = sanitize_chat_instructions(instructions);
    if !instructions.is_empty() {
        sections.push(PromptSection {
            title: "Operating Instructions",
            body: instructions,
        });
    }

    sections.push(PromptSection {
        title: "Runtime Context",
        body: render_runtime_context(cwd, shell, recent_command),
    });

    let workspace_context = join_sections(extra_sections);
    if !workspace_context.is_empty() {
        sections.push(PromptSection {
            title: "Workspace Context",
            body: workspace_context,
        });
    }

    sections.push(PromptSection {
        title: "Current Request",
        body: request.trim().to_string(),
    });

    render_sections(&sections)
}

pub fn build_chat_execution_prompt(
    base_prompt: &str,
    memory_context: &str,
    tools: &[ToolDefinition],
    include_tool_context: bool,
    tool_calling_enabled: bool,
    tool_turns: &[ToolTurnContext],
    task_directive: Option<TaskDirectiveContext>,
) -> String {
    let mut sections = Vec::new();
    if !base_prompt.trim().is_empty() {
        sections.push(base_prompt.trim().to_string());
    }
    if !memory_context.trim().is_empty() {
        sections.push(render_sections(&[PromptSection {
            title: "Memory",
            body: memory_context.trim().to_string(),
        }]));
    }
    if include_tool_context || tool_calling_enabled {
        let catalog = render_tool_catalog(tools);
        if !catalog.is_empty() {
            sections.push(render_sections(&[PromptSection {
                title: "Available Tools",
                body: catalog,
            }]));
        }
    }
    if tool_calling_enabled && !tools.is_empty() {
        sections.push(render_sections(&[PromptSection {
            title: "Tool Use Protocol",
            body: tool_protocol_block(tools),
        }]));
    }
    if !tool_turns.is_empty() {
        sections.push(render_sections(&[PromptSection {
            title: "Tool Results",
            body: render_tool_turns(tool_turns),
        }]));
    }
    if let Some(directive) = task_directive {
        sections.push(render_sections(&[PromptSection {
            title: "Task Directive",
            body: render_task_directive(directive),
        }]));
    }
    sections.join("\n\n")
}

fn tool_protocol_block(_tools: &[ToolDefinition]) -> String {
    format!(
        "You may use daemon tools to complete the task.\n\
Reply in exactly one of these forms:\n\
TOOL: <tool_name> <json arguments>\n\
PLAN: <short summary>\n\
STEP: <tool_name> <json arguments>\n\
STEP: <tool_name> <json arguments>\n\
FINAL: <plain text response>\n\
Rules:\n\
- Use only listed tools.\n\
- Use one tool at a time.\n\
- Choose the tool whose purpose and input schema best match the current request or current task step.\n\
- A bare STEP is valid when you want to provide the next concrete action directly without a separate PLAN line.\n\
- Prefer dedicated primitives like file_read, path_search, glob, grep, web_fetch, and web_search over shell_exec when they fit the task.\n\
- For requests to create, save, overwrite, or otherwise write a local file, prefer file_write directly.\n\
- For requests to modify existing text in a local file, prefer file_edit directly.\n\
- Do not answer a local write request with prose about what you might do next. Choose the concrete write tool step instead.\n\
- Use shell_exec only when no dedicated tool fits or when the task is specifically about running a command.\n\
- Use interactive_shell_start plus interactive_shell_read and interactive_shell_write when the task requires driving an interactive command, REPL, installer, prompt, or TUI over multiple turns.\n\
- Use interactive_shell_key when the next action is a real keypress such as Enter, Tab, Escape, arrow keys, or Ctrl+C.\n\
- When the user asks to use another CLI, REPL, installer, or agent, invoke that target program directly. Do not wrap the user's request back through noodle itself.\n\
- Never invoke oo, ww, or the noodle CLI from shell_exec or interactive_shell_start unless the user explicitly asks to test noodle.\n\
- After starting an interactive shell session, keep using the returned session_id for subsequent interactive_shell_read, interactive_shell_write, and interactive_shell_close calls.\n\
- interactive_shell_write uses terminal semantics: put text to type or paste in text, and set submit=true when you want noodle to press Enter after sending that text.\n\
- For TUIs and highlighted menus, prefer interactive_shell_key with Enter, arrows, Tab, or Escape instead of typing the visible label or number unless the UI is clearly asking for textual input.\n\
- For multi-line prompts to interactive tools, send the full prompt as text and set submit=true once at the end. Do not rely on embedded \\n characters to press Enter.\n\
- For interactive sessions, keep reading until the task is actually complete. A splash screen, prompt, or first screenful of output is not completion.\n\
- For slower interactive agents, the first read may only show startup UI or an input prompt. That does not mean the task is done.\n\
- After you send a substantial prompt to an interactive agent, expect a quiet thinking period and continue polling with interactive_shell_read before concluding that nothing happened.\n\
- interactive_shell_read can use longer waits. For slower CLIs and networked agents, prefer wait_ms in the 2000-10000 range and keep polling until completion markers or process exit.\n\
- Treat a settled interactive screen as a decision point. Inspect the screen contents and choose the next action from what is actually visible, not from fixed prompt characters.\n\
- If an interactive read returns no new output, do not keep rereading the same screen unless you intentionally want to wait longer. Decide what to type, what key to press, or whether to close the session.\n\
- When delegating to another interactive agent or CLI, prefer sending the user's actual task directly. Do not choose onboarding, setup, or initialization commands unless the user explicitly asked for setup or the task truly requires initialization first.\n\
- When an interactive screen clearly shows a prompt, menu, text cursor, question, or other UI waiting for input, treat that as the next action point and continue the session instead of stopping.\n\
- Use interactive_shell_write for textual input and pasted prompts. Use interactive_shell_key for control actions like Enter, Tab, Escape, arrows, Ctrl+C, or confirming an already-highlighted menu choice.\n\
- If the latest interactive_shell_read still shows unresolved UI waiting for input, do not return FINAL yet. Continue the session with interactive_shell_write or interactive_shell_key, or keep reading only if the screen is still changing.\n\
- If an interactive prompt requests secrets, passwords, destructive confirmation, account access, payment, or irreversible changes, stop and ask the user instead of choosing automatically.\n\
- Treat tool outputs as primary data. Do not summarize, compress, or paraphrase file contents unless the user explicitly asks for a summary or explanation.\n\
- If a tool result already satisfies the user's request, return that result directly instead of asking an unnecessary follow-up question.\n\
- For local file, directory, repository, or workspace questions, inspect with local tools before answering.\n\
- For requests to locate a local file or folder by name, prefer path_search first.\n\
- Do not claim a local file or directory is missing until you have checked with tools.\n\
- Prefer local workspace tools for local file and repository tasks.\n\
- For requests to locate named files, folders, worktrees, branches, or personal artifacts, do not assume the current workspace is the only relevant scope.\n\
- If the target may live outside the current workspace, search the broader local roots shown in context before concluding it is missing.\n\
- Prefer web tools only when the task needs outside or current information.\n\
- After tool results are provided, continue toward a final answer.\n\
- Use PLAN when the task clearly needs multiple steps.\n\
- If no tool is needed, respond with FINAL immediately.\n\
Arguments must be valid JSON that matches the selected tool's input schema.",
    )
}

fn render_tool_turns(turns: &[ToolTurnContext]) -> String {
    let last_interactive_read = turns
        .iter()
        .enumerate()
        .rev()
        .find(|(_, turn)| turn.tool == "interactive_shell_read")
        .map(|(index, _)| index);
    turns
        .iter()
        .enumerate()
        .filter(|(index, turn)| {
            turn.tool != "interactive_shell_read" || Some(*index) == last_interactive_read
        })
        .map(|(_, turn)| render_tool_turn(turn))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_turn(turn: &ToolTurnContext) -> String {
    if turn.tool == "interactive_shell_read" {
        return render_interactive_shell_read_turn(turn);
    }
    format!(
        "TOOL_RESULT: {} {} => {}",
        turn.tool,
        compact_json(&turn.args),
        compact_json(&turn.result)
    )
}

fn render_interactive_shell_read_turn(turn: &ToolTurnContext) -> String {
    let output = &turn.result;
    let screen_tail = output
        .get("screen_tail")
        .and_then(Value::as_str)
        .or_else(|| output.get("screen_text").and_then(Value::as_str))
        .or_else(|| output.get("output").and_then(Value::as_str))
        .unwrap_or("")
        .trim_end();
    let prompt_region = output
        .get("prompt_region")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end();
    let menu_options = output
        .get("menu_options")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let status = output
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("running");
    let closed = output
        .get("closed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let prompt_detected = output
        .get("prompt_detected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let idle_ms = output
        .get("idle_ms")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cursor_row = output
        .get("cursor_row")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cursor_col = output
        .get("cursor_col")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let screen = limit_screen_text(screen_tail, 4000);
    let prompt_region = limit_screen_text(prompt_region, 2000);
    let menu_options = limit_screen_text(&menu_options, 800);
    let screen_block = if screen.is_empty() {
        "<empty>"
    } else {
        &screen
    };
    let prompt_block = if prompt_region.is_empty() {
        "<none>"
    } else {
        &prompt_region
    };
    let menu_block = if menu_options.is_empty() {
        "<none>"
    } else {
        &menu_options
    };
    format!(
        "TOOL_RESULT: interactive_shell_read {}\nstatus={} closed={} prompt_detected={} idle_ms={} cursor=({}, {})\nprompt_region:\n{}\n[end prompt_region]\nmenu_options:\n{}\n[end menu_options]\nscreen_tail:\n{}\n[end screen_tail]",
        compact_json(&turn.args),
        status,
        closed,
        prompt_detected,
        idle_ms,
        cursor_row,
        cursor_col,
        prompt_block,
        menu_block,
        screen_block
    )
}

fn render_task_directive(directive: TaskDirectiveContext) -> String {
    match directive {
        TaskDirectiveContext::Finalize { summary } => {
            format!(
                "Planned task summary: {}\nTask finished. Respond with FINAL only.\nIf more tool work is still required, reply with TOOL, STEP, or PLAN instead of forcing a final answer.",
                summary
            )
        }
        TaskDirectiveContext::ContinueInteractive { summary } => {
            if summary.trim().is_empty() {
                "An interactive shell session is still open and the screen has settled.\nDo not answer the user yet.\nLook at the latest interactive_shell_read result, especially prompt_region, menu_options, and screen_tail.\nChoose exactly one next TOOL or STEP to continue the terminal session.\nIf the screen is waiting for text, use interactive_shell_write.\nIf the screen is waiting for a keypress or has the desired option highlighted already, use interactive_shell_key.\nUse interactive_shell_read only when the screen is still changing or you intentionally need to wait longer.\nOnly return FINAL after the interactive session is complete or explicitly closed.".to_string()
            } else {
                format!(
                    "Planned task summary: {}\nAn interactive shell session is still open and the screen has settled.\nDo not answer the user yet.\nLook at the latest interactive_shell_read result, especially prompt_region, menu_options, and screen_tail.\nChoose exactly one next TOOL or STEP to continue the terminal session.\nIf the screen is waiting for text, use interactive_shell_write.\nIf the screen is waiting for a keypress or has the desired option highlighted already, use interactive_shell_key.\nUse interactive_shell_read only when the screen is still changing or you intentionally need to wait longer.\nOnly return FINAL after the interactive session is complete or explicitly closed.",
                    summary
                )
            }
        }
        TaskDirectiveContext::VerifyDirectAnswer {
            draft_answer,
            required_tool_use_reason,
        } => {
            let mut directive = format!(
                "You previously drafted a direct answer before using any tools.\n\
Draft answer:\n{}\n\n\
Treat that draft as unverified.\n\
Decide whether it is safe to return without using tools.\n\
- If the draft makes any claim about local files, directories, repositories, workspace contents, command availability, shell state, or whether something exists or is missing, you must verify with tools first.\n\
- For location-style requests, make sure you checked an appropriate scope, not just the current workspace, before concluding something is missing.\n\
- Never reply FINAL_OK for a negative local claim like \"not found\", \"missing\", or \"does not exist\" unless tool results already established it.\n\
- If you are unsure, use tools.\n\
- If the draft is acceptable as-is and no tool use is needed, reply exactly: FINAL_OK\n\
- If you need to inspect, verify, search, or otherwise use tools first, reply with exactly one of:\n\
TOOL: <tool_name> <json arguments>\n\
STEP: <tool_name> <json arguments>\n\
PLAN: <short summary>\n\
Do not answer the user directly in this pass.",
                draft_answer
            );
            if let Some(reason) = required_tool_use_reason.filter(|value| !value.trim().is_empty())
            {
                directive.push_str(&format!(
                    "\n- This request explicitly requires tool use before any final answer: {}",
                    reason
                ));
                directive.push_str(
                    "\n- Do not reply FINAL_OK for this request.\n- Do not reply with a meta statement about needing tools. Choose the next tool step now.",
                );
            }
            directive
        }
        TaskDirectiveContext::ForceToolChoice { reason } => format!(
            "You must choose a tool-based next action now.\nReason: {}\n\
Reply with exactly one of:\n\
TOOL: <tool_name> <json arguments>\n\
STEP: <tool_name> <json arguments>\n\
PLAN: <short summary>\n\
Do not reply with FINAL or FINAL_OK.",
            reason
        ),
        TaskDirectiveContext::Replan {
            summary,
            goal,
            failed_step_tool,
            failed_step_args,
            error,
        } => format!(
            "Planned task summary: {}\nA planned task step failed.\nOriginal goal: {}\nFailed step: {} {}\nError: {}\nReturn either:\nPLAN: <short summary>\nSTEP: <tool_name> <json arguments>\nFINAL: <plain text response>",
            summary,
            goal,
            failed_step_tool,
            compact_json(&failed_step_args),
            error
        ),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

fn limit_screen_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = value
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...[truncated]\n{keep}")
}

fn render_tool_catalog(tools: &[ToolDefinition]) -> String {
    tools
        .iter()
        .map(|tool| {
            format!(
                "- {} [{} / {}]\n  Purpose: {}\n  Input JSON schema: {}",
                tool.name,
                tool.tier.as_str(),
                tool.permission.as_str(),
                tool.description,
                compact_json(&tool.input_schema)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg_attr(not(test), allow(dead_code))]
fn sanitize_chat_instructions(instructions: &str) -> String {
    let lines = instructions
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.contains("{user_input}") {
                return None;
            }
            Some(trimmed)
        })
        .collect::<Vec<_>>();
    lines.join("\n")
}

#[cfg_attr(not(test), allow(dead_code))]
fn render_runtime_context(cwd: &str, shell: &str, recent_command: &str) -> String {
    let mut lines = vec![
        format!("Current directory: {cwd}"),
        format!("Shell: {shell}"),
    ];
    if !recent_command.trim().is_empty() {
        lines.push(format!("Recent command: {}", recent_command.trim()));
    }
    lines.join("\n")
}

fn join_sections(sections: &[String]) -> String {
    sections
        .iter()
        .filter_map(|section| {
            let trimmed = section.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_sections(sections: &[PromptSection]) -> String {
    sections
        .iter()
        .filter_map(|section| {
            let body = section.body.trim();
            if body.is_empty() {
                None
            } else {
                Some(format!("[{}]\n{}", section.title, body))
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::{
        EventPromptInput, TaskDirectiveContext, ToolTurnContext, build_chat_base_prompt,
        build_chat_execution_prompt, build_event_prompt,
    };
    use crate::tooling::{ToolDefinition, ToolPermissionClass, ToolTier};
    use serde_json::json;

    fn sample_tool() -> ToolDefinition {
        ToolDefinition {
            name: "file_read",
            description: "Read a local file.",
            tier: ToolTier::Tier1,
            permission: ToolPermissionClass::ReadOnly,
            input_schema: json!({}),
        }
    }

    #[test]
    fn event_prompt_includes_soul_substitutions_and_extra_sections() {
        let extra = vec!["Extra context".to_string()];
        let prompt = build_event_prompt(EventPromptInput {
            mode: "chat",
            template: "Question: {user_input}\nmode={mode}\ncwd={cwd}\nshell={shell}\nexit={exit_status}\nrecent={recent_command}",
            input: "hello",
            cwd: "/tmp/demo",
            shell: "zsh",
            exit_status: 127,
            recent_command: "badcmd",
            soul: Some("You are noodle."),
            extra_sections: &extra,
        });
        assert!(prompt.contains("[Identity]"));
        assert!(prompt.contains("You are noodle."));
        assert!(prompt.contains("[Operating Instructions]"));
        assert!(prompt.contains("Question: hello"));
        assert!(prompt.contains("mode=chat"));
        assert!(prompt.contains("[Additional Context]"));
        assert!(prompt.contains("Extra context"));
    }

    #[test]
    fn chat_base_prompt_splits_instructions_runtime_workspace_and_request() {
        let extra = vec![
            "Workspace root: /tmp/demo".to_string(),
            "Git repository: yes".to_string(),
        ];
        let prompt = build_chat_base_prompt(
            "You are noodle.\nQuestion: {user_input}\n",
            "inspect this repo",
            "/tmp/demo",
            "zsh",
            "rg todo",
            Some("Soul block"),
            &extra,
        );
        assert!(prompt.contains("[Identity]"));
        assert!(prompt.contains("Soul block"));
        assert!(prompt.contains("[Operating Instructions]"));
        assert!(prompt.contains("You are noodle."));
        assert!(!prompt.contains("{user_input}"));
        assert!(!prompt.contains("Question:"));
        assert!(prompt.contains("[Runtime Context]"));
        assert!(prompt.contains("Current directory: /tmp/demo"));
        assert!(prompt.contains("[Workspace Context]"));
        assert!(prompt.contains("Workspace root: /tmp/demo"));
        assert!(prompt.contains("[Current Request]"));
        assert!(prompt.contains("inspect this repo"));
    }

    #[test]
    fn chat_execution_prompt_includes_memory_tools_turns_and_task_directive() {
        let turns = vec![ToolTurnContext {
            tool: "file_read".into(),
            args: json!({"path":"/tmp/x"}),
            result: json!({"content":"ok"}),
        }];
        let prompt = build_chat_execution_prompt(
            "Question: inspect the file",
            "Compiled memory:\n- user prefers concise answers",
            &[sample_tool()],
            true,
            true,
            &turns,
            Some(TaskDirectiveContext::Replan {
                summary: "inspect then summarize".into(),
                goal: "inspect broken file".into(),
                failed_step_tool: "file_read".into(),
                failed_step_args: json!({"path":"/tmp/missing"}),
                error: "No such file".into(),
            }),
        );
        assert!(prompt.contains("[Memory]"));
        assert!(prompt.contains("Compiled memory"));
        assert!(prompt.contains("[Available Tools]"));
        assert!(prompt.contains("Input JSON schema"));
        assert!(prompt.contains("[Tool Use Protocol]"));
        assert!(prompt.contains("Choose the tool whose purpose and input schema best match"));
        assert!(prompt.contains("current workspace is the only relevant scope"));
        assert!(prompt.contains("broader local roots shown in context"));
        assert!(prompt.contains("[Tool Results]"));
        assert!(prompt.contains("TOOL_RESULT: file_read"));
        assert!(prompt.contains("[Task Directive]"));
        assert!(prompt.contains("Original goal: inspect broken file"));
        assert!(prompt.contains("Error: No such file"));
    }

    #[test]
    fn interactive_shell_tool_turns_prioritize_prompt_region_and_screen_tail() {
        let banner = "banner line\n".repeat(600);
        let turns = vec![ToolTurnContext {
            tool: "interactive_shell_read".into(),
            args: json!({"session_id":"shell-1"}),
            result: json!({
                "status": "running",
                "closed": false,
                "prompt_detected": true,
                "idle_ms": 100,
                "cursor_row": 40,
                "cursor_col": 1,
                "screen_tail": format!("{banner}Do you want to proceed?\n1. Yes\n2. No"),
                "prompt_region": "Do you want to proceed?\n1. Yes\n2. No",
                "menu_options": ["1. Yes", "2. No"],
            }),
        }];
        let prompt = build_chat_execution_prompt(
            "Question: continue the interactive task",
            "",
            &[sample_tool()],
            true,
            true,
            &turns,
            None,
        );
        assert!(prompt.contains("prompt_region:\nDo you want to proceed?"));
        assert!(prompt.contains("menu_options:\n1. Yes\n2. No"));
        assert!(prompt.contains("...[truncated]"));
        assert!(prompt.contains("screen_tail:"));
    }
}
