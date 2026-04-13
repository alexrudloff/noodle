use crate::actions::DaemonAction;
use crate::context_builder::{TaskDirectiveContext, ToolTurnContext, build_chat_execution_prompt};
use crate::permissions::{
    ChatExecutionSnapshot, PendingContinuation, PendingPermissionRequest, ToolTurnSnapshot,
    clear_pending_permission, load_pending_permission, persist_pending_permission,
};
use crate::planner::{PlannedChatStep, TaskPlan, TaskStep, parse_planned_chat_step};
use crate::tasks::{
    TaskRecord, TaskRuntimeState, clear_task_runtime_state, load_task_runtime_state,
    persist_task_record, persist_task_runtime_state,
};
use crate::tooling::{
    ToolCallResult, ToolDefinition, ToolPermissionDecision, invoke_builtin_tool,
    permission_decision_for_tool, prepare_builtin_tool_args, tool_definition_by_name,
};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ChatExecutionConfig {
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
    pub available_tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone)]
struct ToolTurn {
    tool: String,
    args: Value,
    result: ToolCallResult,
}

pub fn run_chat_execution(
    config: &Value,
    request: ChatExecutionConfig,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    if !request.tool_calling_enabled || request.available_tools.is_empty() {
        let prompt = build_chat_execution_prompt(
            &request.base_prompt,
            &request.memory_context,
            &request.available_tools,
            request.include_tool_context,
            false,
            &[],
            None,
        );
        let response = model_output(&prompt)?;
        return Ok(DaemonAction::Message {
            plugin: request.plugin,
            message: clean_text(&response),
        });
    }

    execute_tool_loop(
        config,
        &request,
        Vec::new(),
        request.max_tool_rounds,
        Vec::new(),
        streaming,
        emitter,
        model_output,
    )
}

pub fn resume_chat_execution_from_permission(
    config: &Value,
    permission_id: &str,
    decision: &str,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    let Some(pending) = load_pending_permission(config, permission_id)? else {
        return Ok(DaemonAction::Ask {
            plugin: "chat".into(),
            question: "That permission request no longer exists.".into(),
        });
    };
    clear_pending_permission(config, permission_id)?;

    if !decision.eq_ignore_ascii_case("allow") {
        if let PendingContinuation::Plan { mut task, .. } = pending.continuation.clone() {
            task.status = "permission_denied".into();
            persist_task_record(config, &task)?;
        }
        return Ok(DaemonAction::Message {
            plugin: pending.plugin,
            message: format!("Permission denied for {}.", pending.tool),
        });
    }

    let request = snapshot_to_request(&pending.request);
    let mut turns = snapshots_to_turns(&pending.tool_turns);
    let mut progress = Vec::new();
    if let Some(action) = emit_tool_running(
        emitter,
        &request,
        &pending.pending_step.tool,
        &pending.pending_step.args,
        streaming,
    )? {
        progress.push(action);
    }
    let normalized_args = prepare_builtin_tool_args(
        &pending.pending_step.tool,
        &pending.pending_step.args,
        Some(&request.working_directory),
    );
    let result = invoke_builtin_tool(
        config,
        Some(&request.working_directory),
        &pending.pending_step.tool,
        &pending.pending_step.args,
    )?;
    turns.push(ToolTurn {
        tool: pending.pending_step.tool.clone(),
        args: normalized_args,
        result,
    });
    if let Some(action) = emit_tool_done(
        emitter,
        &request,
        &pending.pending_step.tool,
        &pending.pending_step.args,
        turns
            .last()
            .map(|turn| &turn.result)
            .expect("tool turn exists"),
        streaming,
    )? {
        progress.push(action);
    }
    let tool_done_summary_text = tool_done_summary(
        &pending.pending_step.tool,
        &pending.pending_step.args,
        turns
            .last()
            .map(|turn| &turn.result)
            .expect("tool turn exists"),
    );

    match pending.continuation {
        PendingContinuation::ToolLoop { remaining_rounds } => execute_tool_loop(
            config,
            &request,
            turns,
            remaining_rounds,
            progress,
            streaming,
            emitter,
            model_output,
        ),
        PendingContinuation::Plan {
            mut task,
            current_step_index,
            remaining_steps,
            replans_remaining,
        } => {
            let progress = vec![
                DaemonAction::TaskStep {
                    plugin: request.plugin.clone(),
                    task_id: task.id.clone(),
                    index: current_step_index + 1,
                    total: task.steps.len(),
                    tool: pending.pending_step.tool.clone(),
                    status: "running".into(),
                    summary: tool_running_summary(
                        &pending.pending_step.tool,
                        &pending.pending_step.args,
                    ),
                },
                DaemonAction::TaskStep {
                    plugin: request.plugin.clone(),
                    task_id: task.id.clone(),
                    index: current_step_index + 1,
                    total: task.steps.len(),
                    tool: pending.pending_step.tool.clone(),
                    status: "done".into(),
                    summary: tool_done_summary_text,
                },
            ];
            emit_action(emitter, &progress[0], streaming)?;
            emit_action(emitter, &progress[1], streaming)?;
            let next_index = current_step_index.saturating_add(1);
            let next_steps = remaining_steps.into_iter().skip(1).collect();
            execute_plan_remaining(
                config,
                &request,
                &mut task,
                next_index,
                next_steps,
                turns,
                progress,
                replans_remaining,
                streaming,
                emitter,
                model_output,
            )
        }
    }
}

pub fn resume_task_execution(
    config: &Value,
    task_id: &str,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    let Some(state) = load_task_runtime_state(config, task_id)? else {
        return Ok(DaemonAction::Ask {
            plugin: "chat".into(),
            question: format!("Task {} has no resumable runtime state.", task_id),
        });
    };
    let request = request_from_value(&state.request)?;
    let turns = tool_turns_from_values(&state.tool_turns)?;
    let mut task = state.task;
    let progress = vec![DaemonAction::TaskStarted {
        plugin: request.plugin.clone(),
        task_id: task.id.clone(),
        summary: format!("Resuming {}", task.summary),
    }];
    emit_action(
        emitter,
        progress.last().expect("task started exists"),
        streaming,
    )?;
    execute_plan_remaining(
        config,
        &request,
        &mut task,
        state.current_step_index,
        state.remaining_steps,
        turns,
        progress,
        state.replans_remaining.max(1),
        streaming,
        emitter,
        model_output,
    )
}

fn execute_tool_loop(
    config: &Value,
    request: &ChatExecutionConfig,
    mut turns: Vec<ToolTurn>,
    remaining_rounds: usize,
    mut progress: Vec<DaemonAction>,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    if remaining_rounds == 0 {
        if interactive_session_needs_observation(&turns) {
            let autoread = maybe_autoread_active_interactive_session(
                config,
                request,
                &mut turns,
                &mut progress,
                streaming,
                emitter,
                true,
            )?;
            if autoread.progressed {
                return execute_tool_loop(
                    config,
                    request,
                    turns,
                    1,
                    progress,
                    streaming,
                    emitter,
                    model_output,
                );
            }
            if autoread.still_running {
                maybe_close_active_interactive_session(config, request, &mut turns)?;
                return Ok(finalize_action(
                    progress,
                    DaemonAction::Ask {
                        plugin: request.plugin.clone(),
                        question: "The interactive command is still running but has not produced a final result yet. Ask me to keep waiting or try a different approach.".into(),
                    },
                    streaming,
                ));
            }
        }
        if interactive_prompt_pending(&turns) {
            return match request_interactive_continuation(request, &turns, None, model_output)? {
                PlannedChatStep::Tool(step) => continue_tool_loop_with_step(
                    config,
                    request,
                    turns,
                    progress,
                    1,
                    streaming,
                    emitter,
                    model_output,
                    step,
                ),
                PlannedChatStep::Plan(plan) => {
                    if !request.task_execution_enabled {
                        Ok(finalize_action(
                            progress,
                            DaemonAction::Ask {
                                plugin: request.plugin.clone(),
                                question: "That request needs multi-step planning, but task execution is disabled.".into(),
                            },
                            streaming,
                        ))
                    } else {
                        execute_plan(
                            config,
                            request,
                            plan,
                            turns,
                            request.max_replans,
                            progress,
                            streaming,
                            emitter,
                            model_output,
                        )
                    }
                }
                PlannedChatStep::Final(_) => Ok(finalize_action(
                    progress,
                    DaemonAction::Ask {
                        plugin: request.plugin.clone(),
                        question: "The interactive command is waiting for input and I need to continue it before answering you.".into(),
                    },
                    streaming,
                )),
            };
        }
        let turn_contexts = tool_turn_contexts(&turns);
        let response = model_output(&build_chat_execution_prompt(
            &request.base_prompt,
            &request.memory_context,
            &request.available_tools,
            request.include_tool_context,
            request.tool_calling_enabled,
            &turn_contexts,
            None,
        ))?;
        return Ok(finalize_action(
            progress,
            DaemonAction::Message {
                plugin: request.plugin.clone(),
                message: clean_text(&response),
            },
            streaming,
        ));
    }

    for offset in 0..remaining_rounds {
        let turn_contexts = tool_turn_contexts(&turns);
        let response = model_output(&build_chat_execution_prompt(
            &request.base_prompt,
            &request.memory_context,
            &request.available_tools,
            request.include_tool_context,
            request.tool_calling_enabled,
            &turn_contexts,
            if interactive_prompt_pending(&turns) {
                Some(TaskDirectiveContext::ContinueInteractive {
                    summary: String::new(),
                })
            } else {
                None
            },
        ))?;
        match parse_planned_chat_step(&response) {
            PlannedChatStep::Final(text) => {
                if interactive_session_needs_observation(&turns) {
                    let autoread = maybe_autoread_active_interactive_session(
                        config,
                        request,
                        &mut turns,
                        &mut progress,
                        streaming,
                        emitter,
                        true,
                    )?;
                    if autoread.progressed {
                        continue;
                    }
                    if autoread.still_running {
                        maybe_close_active_interactive_session(config, request, &mut turns)?;
                        return Ok(finalize_action(
                            progress,
                            DaemonAction::Ask {
                                plugin: request.plugin.clone(),
                                question: "The interactive command is still running and has not produced a settled screen yet. Ask me to keep waiting or try a different approach.".into(),
                            },
                            streaming,
                        ));
                    }
                }
                if interactive_prompt_pending(&turns) {
                    match request_interactive_continuation(request, &turns, None, model_output)? {
                        PlannedChatStep::Tool(step) => {
                            return continue_tool_loop_with_step(
                                config,
                                request,
                                turns,
                                progress,
                                remaining_rounds.saturating_sub(offset + 1),
                                streaming,
                                emitter,
                                model_output,
                                step,
                            );
                        }
                        PlannedChatStep::Plan(plan) => {
                            if !request.task_execution_enabled {
                                return Ok(finalize_action(
                                    progress,
                                    DaemonAction::Ask {
                                        plugin: request.plugin.clone(),
                                        question: "That request needs multi-step planning, but task execution is disabled.".into(),
                                    },
                                    streaming,
                                ));
                            }
                            return execute_plan(
                                config,
                                request,
                                plan,
                                turns,
                                request.max_replans,
                                progress,
                                streaming,
                                emitter,
                                model_output,
                            );
                        }
                        PlannedChatStep::Final(_) => {
                            return Ok(finalize_action(
                                progress,
                                DaemonAction::Ask {
                                    plugin: request.plugin.clone(),
                                    question: "The interactive command is waiting for input and I need to continue it before answering you.".into(),
                                },
                                streaming,
                            ));
                        }
                    }
                }
                if text.trim().is_empty() {
                    let autoread = maybe_autoread_active_interactive_session(
                        config,
                        request,
                        &mut turns,
                        &mut progress,
                        streaming,
                        emitter,
                        false,
                    )?;
                    if autoread.progressed {
                        continue;
                    }
                    if autoread.still_running {
                        maybe_close_active_interactive_session(config, request, &mut turns)?;
                        return Ok(finalize_action(
                            progress,
                            DaemonAction::Ask {
                                plugin: request.plugin.clone(),
                                question: "The interactive command is still running but has not produced a final result yet. Ask me to keep waiting or try a different approach.".into(),
                            },
                            streaming,
                        ));
                    }
                }
                if turns.is_empty() {
                    return verify_direct_final(
                        config,
                        request,
                        turns,
                        remaining_rounds.saturating_sub(offset + 1),
                        progress,
                        streaming,
                        emitter,
                        text,
                        model_output,
                    );
                }
                maybe_close_active_interactive_session(config, request, &mut turns)?;
                return Ok(finalize_action(
                    progress,
                    DaemonAction::Message {
                        plugin: request.plugin.clone(),
                        message: text,
                    },
                    streaming,
                ));
            }
            PlannedChatStep::Tool(TaskStep { tool: name, args }) => {
                return continue_tool_loop_with_step(
                    config,
                    request,
                    turns,
                    progress,
                    remaining_rounds.saturating_sub(offset + 1),
                    streaming,
                    emitter,
                    model_output,
                    TaskStep { tool: name, args },
                );
            }
            PlannedChatStep::Plan(plan) => {
                if !request.task_execution_enabled {
                    return Ok(finalize_action(
                        progress,
                        DaemonAction::Ask {
                            plugin: request.plugin.clone(),
                            question: "That request needs multi-step planning, but task execution is disabled.".into(),
                        },
                        streaming,
                    ));
                }
                return execute_plan(
                    config,
                    request,
                    plan,
                    turns,
                    request.max_replans,
                    progress,
                    streaming,
                    emitter,
                    model_output,
                );
            }
        }
    }

    let turn_contexts = tool_turn_contexts(&turns);
    let response = model_output(&build_chat_execution_prompt(
        &request.base_prompt,
        &request.memory_context,
        &request.available_tools,
        request.include_tool_context,
        request.tool_calling_enabled,
        &turn_contexts,
        None,
    ))?;
    Ok(finalize_action(
        progress,
        DaemonAction::Message {
            plugin: request.plugin.clone(),
            message: clean_text(&response),
        },
        streaming,
    ))
}

fn continue_tool_loop_with_step(
    config: &Value,
    request: &ChatExecutionConfig,
    mut turns: Vec<ToolTurn>,
    mut progress: Vec<DaemonAction>,
    remaining_rounds: usize,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
    step: TaskStep,
) -> Result<DaemonAction, String> {
    let TaskStep { tool: name, args } = step;
    let allowed = request.available_tools.iter().any(|tool| tool.name == name);
    if !allowed {
        return Ok(finalize_action(
            progress,
            DaemonAction::Ask {
                plugin: request.plugin.clone(),
                question: format!("That request needs a tool that is not available: {name}"),
            },
            streaming,
        ));
    }
    if let Some(reason) = recursive_self_invocation_reason(request, &name, &args) {
        let action = DaemonAction::ToolStep {
            plugin: request.plugin.clone(),
            tool: name.clone(),
            status: "failed".into(),
            summary: reason.clone(),
        };
        emit_action(emitter, &action, streaming)?;
        progress.push(action);
        turns.push(ToolTurn {
            tool: name.clone(),
            args: prepare_builtin_tool_args(&name, &args, Some(&request.working_directory)),
            result: ToolCallResult {
                tool: name,
                ok: false,
                output: json!({ "error": reason, "refused": true }),
            },
        });
        return execute_tool_loop(
            config,
            request,
            turns,
            remaining_rounds,
            progress,
            streaming,
            emitter,
            model_output,
        );
    }
    if let Some(action) = maybe_request_permission(
        config,
        request,
        &turns,
        TaskStep {
            tool: name.clone(),
            args: args.clone(),
        },
        PendingContinuation::ToolLoop { remaining_rounds },
    )? {
        return Ok(finalize_action(progress, action, streaming));
    }
    if let Some(action) = emit_tool_running(emitter, request, &name, &args, streaming)? {
        progress.push(action);
    }
    execute_tool_step_inline(
        config,
        request,
        &mut turns,
        &mut progress,
        streaming,
        emitter,
        &name,
        &args,
    )?;
    if let Some(action) = maybe_return_raw_tool_result(request, &turns) {
        return Ok(finalize_action(progress, action, streaming));
    }
    execute_tool_loop(
        config,
        request,
        turns,
        remaining_rounds,
        progress,
        streaming,
        emitter,
        model_output,
    )
}

fn maybe_return_raw_tool_result(
    request: &ChatExecutionConfig,
    turns: &[ToolTurn],
) -> Option<DaemonAction> {
    if turns.len() != 1 {
        return None;
    }
    let turn = turns.first()?;
    if turn.tool != "file_read" {
        return None;
    }
    let content = turn.result.output.get("content").and_then(Value::as_str)?;
    if content.len() > 20_000 && !request_explicitly_wants_exact_output(&request.input) {
        return None;
    }
    if request_explicitly_wants_summary(&request.input) {
        return None;
    }
    Some(DaemonAction::Message {
        plugin: request.plugin.clone(),
        message: content.to_string(),
    })
}

fn verify_direct_final(
    config: &Value,
    request: &ChatExecutionConfig,
    turns: Vec<ToolTurn>,
    remaining_rounds: usize,
    progress: Vec<DaemonAction>,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    direct_text: String,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    if !request.tool_calling_enabled || request.available_tools.is_empty() {
        return Ok(finalize_action(
            progress,
            DaemonAction::Message {
                plugin: request.plugin.clone(),
                message: direct_text,
            },
            streaming,
        ));
    }

    let required_tool_use_reason = required_tool_use_reason(request);
    let prompt = build_chat_execution_prompt(
        &request.base_prompt,
        &request.memory_context,
        &request.available_tools,
        request.include_tool_context,
        request.tool_calling_enabled,
        &[],
        Some(TaskDirectiveContext::VerifyDirectAnswer {
            draft_answer: direct_text.clone(),
            required_tool_use_reason: required_tool_use_reason.clone(),
        }),
    );
    let verification_raw = model_output(&prompt)?;
    let verification = clean_text(&verification_raw);
    if verification.eq_ignore_ascii_case("FINAL_OK") {
        if let Some(reason) = required_tool_use_reason {
            return force_tool_choice_after_direct_final(
                config,
                request,
                turns,
                remaining_rounds,
                progress,
                streaming,
                emitter,
                reason,
                model_output,
            );
        }
        return Ok(finalize_action(
            progress,
            DaemonAction::Message {
                plugin: request.plugin.clone(),
                message: direct_text,
            },
            streaming,
        ));
    }

    match parse_planned_chat_step(&verification_raw) {
        PlannedChatStep::Final(_) => {
            if let Some(reason) = required_tool_use_reason {
                return force_tool_choice_after_direct_final(
                    config,
                    request,
                    turns,
                    remaining_rounds,
                    progress,
                    streaming,
                    emitter,
                    reason,
                    model_output,
                );
            }
            Ok(finalize_action(
                progress,
                DaemonAction::Ask {
                    plugin: request.plugin.clone(),
                    question: "I need to inspect with tools before I can answer that confidently."
                        .into(),
                },
                streaming,
            ))
        }
        PlannedChatStep::Tool(TaskStep { tool: name, args }) => {
            let allowed = request.available_tools.iter().any(|tool| tool.name == name);
            if !allowed {
                return Ok(finalize_action(
                    progress,
                    DaemonAction::Ask {
                        plugin: request.plugin.clone(),
                        question: format!(
                            "That request needs a tool that is not available: {name}"
                        ),
                    },
                    streaming,
                ));
            }
            if let Some(reason) = recursive_self_invocation_reason(request, &name, &args) {
                let mut next_progress = progress;
                let action = DaemonAction::ToolStep {
                    plugin: request.plugin.clone(),
                    tool: name.clone(),
                    status: "failed".into(),
                    summary: reason.clone(),
                };
                emit_action(emitter, &action, streaming)?;
                next_progress.push(action);
                let mut new_turns = turns;
                new_turns.push(ToolTurn {
                    tool: name.clone(),
                    args: prepare_builtin_tool_args(&name, &args, Some(&request.working_directory)),
                    result: ToolCallResult {
                        tool: name,
                        ok: false,
                        output: json!({ "error": reason, "refused": true }),
                    },
                });
                return Ok(execute_tool_loop(
                    config,
                    request,
                    new_turns,
                    remaining_rounds,
                    next_progress,
                    streaming,
                    emitter,
                    model_output,
                )?);
            }
            if let Some(action) = maybe_request_permission(
                config,
                request,
                &turns,
                TaskStep {
                    tool: name.clone(),
                    args: args.clone(),
                },
                PendingContinuation::ToolLoop { remaining_rounds },
            )? {
                return Ok(finalize_action(progress, action, streaming));
            }
            let mut next_progress = progress;
            if let Some(action) = emit_tool_running(emitter, request, &name, &args, streaming)? {
                next_progress.push(action);
            }
            let normalized_args =
                prepare_builtin_tool_args(&name, &args, Some(&request.working_directory));
            let result =
                invoke_builtin_tool(config, Some(&request.working_directory), &name, &args)?;
            if let Some(action) = emit_tool_done(
                emitter,
                request,
                &name,
                &normalized_args,
                &result,
                streaming,
            )? {
                next_progress.push(action);
            }
            let mut new_turns = turns;
            new_turns.push(ToolTurn {
                tool: name,
                args: normalized_args,
                result,
            });
            Ok(execute_tool_loop(
                config,
                request,
                new_turns,
                remaining_rounds,
                next_progress,
                streaming,
                emitter,
                model_output,
            )?)
        }
        PlannedChatStep::Plan(plan) => {
            if !request.task_execution_enabled {
                return Ok(finalize_action(
                    progress,
                    DaemonAction::Ask {
                        plugin: request.plugin.clone(),
                        question:
                            "That request needs multi-step planning, but task execution is disabled."
                                .into(),
                    },
                    streaming,
                ));
            }
            Ok(execute_plan(
                config,
                request,
                plan,
                turns,
                request.max_replans,
                progress,
                streaming,
                emitter,
                model_output,
            )?)
        }
    }
}

fn force_tool_choice_after_direct_final(
    config: &Value,
    request: &ChatExecutionConfig,
    turns: Vec<ToolTurn>,
    remaining_rounds: usize,
    progress: Vec<DaemonAction>,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    reason: String,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    let turn_contexts = tool_turn_contexts(&turns);
    let prompt = build_chat_execution_prompt(
        &request.base_prompt,
        &request.memory_context,
        &request.available_tools,
        request.include_tool_context,
        request.tool_calling_enabled,
        &turn_contexts,
        Some(TaskDirectiveContext::ForceToolChoice {
            reason: reason.clone(),
        }),
    );
    let forced_raw = model_output(&prompt)?;
    match parse_planned_chat_step(&forced_raw) {
        PlannedChatStep::Tool(step) => continue_tool_loop_with_step(
            config,
            request,
            turns,
            progress,
            remaining_rounds,
            streaming,
            emitter,
            model_output,
            step,
        ),
        PlannedChatStep::Plan(plan) => {
            if !request.task_execution_enabled {
                return Ok(finalize_action(
                    progress,
                    DaemonAction::Ask {
                        plugin: request.plugin.clone(),
                        question: "That request needs multi-step planning, but task execution is disabled.".into(),
                    },
                    streaming,
                ));
            }
            execute_plan(
                config,
                request,
                plan,
                turns,
                request.max_replans,
                progress,
                streaming,
                emitter,
                model_output,
            )
        }
        PlannedChatStep::Final(_) => Ok(finalize_action(
            progress,
            DaemonAction::Ask {
                plugin: request.plugin.clone(),
                question: reason,
            },
            streaming,
        )),
    }
}

fn request_explicitly_wants_exact_output(input: &str) -> bool {
    let text = input.to_lowercase();
    [
        "exact",
        "exactly",
        "verbatim",
        "raw",
        "full contents",
        "full content",
        "entire file",
        "whole file",
        "as-is",
        "as is",
        "quote it",
        "share it exactly",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn request_explicitly_wants_summary(input: &str) -> bool {
    let text = input.to_lowercase();
    [
        "summarize",
        "summary",
        "brief",
        "briefly",
        "overview",
        "gist",
        "tldr",
        "tl;dr",
        "explain",
        "explanation",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn request_likely_needs_local_write(input: &str) -> bool {
    let text = input.to_lowercase();
    if [
        "how do i",
        "how to",
        "what command",
        "show me the command",
        "which command",
        "example command",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    {
        return false;
    }
    let has_write_verb = [
        "create",
        "write",
        "save",
        "append",
        "edit",
        "update",
        "modify",
        "rewrite",
        "replace",
        "overwrite",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    if !has_write_verb {
        return false;
    }
    [
        " file",
        " files",
        ".md",
        ".txt",
        ".json",
        ".yaml",
        ".yml",
        ".toml",
        ".rs",
        ".py",
        ".js",
        ".ts",
        ".tsx",
        ".jsx",
        "readme",
        "package.json",
        "config",
        "document",
        "note",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn required_tool_use_reason(request: &ChatExecutionConfig) -> Option<String> {
    let has_write_tool = request
        .available_tools
        .iter()
        .any(|tool| matches!(tool.name, "file_write" | "file_edit"));
    if has_write_tool && request_likely_needs_local_write(&request.input) {
        return Some(
            "The user asked you to create or modify a local file, so you must choose a local write tool before responding."
                .into(),
        );
    }
    None
}

fn request_explicitly_targets_noodle(input: &str) -> bool {
    let text = input.to_lowercase();
    [
        "noodle",
        "command_not_found",
        "command not found",
        "test noodle",
        "debug noodle",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn execute_plan(
    config: &Value,
    request: &ChatExecutionConfig,
    plan: TaskPlan,
    turns: Vec<ToolTurn>,
    replans_remaining: usize,
    mut progress: Vec<DaemonAction>,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    let mut task = TaskRecord::from_plan(&request.plugin, &request.input, &plan);
    persist_task_record(config, &task)?;
    persist_task_runtime_state(
        config,
        &TaskRuntimeState {
            task: task.clone(),
            request: request_to_value(request)?,
            current_step_index: 0,
            remaining_steps: plan.steps.clone(),
            tool_turns: tool_turns_to_values(&turns),
            replans_remaining,
            status: task.status.clone(),
        },
    )?;
    let started = DaemonAction::TaskStarted {
        plugin: request.plugin.clone(),
        task_id: task.id.clone(),
        summary: task.summary.clone(),
    };
    emit_action(emitter, &started, streaming)?;
    progress.push(started);

    execute_plan_remaining(
        config,
        request,
        &mut task,
        0,
        plan.steps,
        turns,
        progress,
        replans_remaining,
        streaming,
        emitter,
        model_output,
    )
}

fn execute_plan_remaining(
    config: &Value,
    request: &ChatExecutionConfig,
    task: &mut TaskRecord,
    start_index: usize,
    remaining_steps: Vec<TaskStep>,
    mut turns: Vec<ToolTurn>,
    mut progress: Vec<DaemonAction>,
    replans_remaining: usize,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    for (offset, step) in remaining_steps.iter().enumerate() {
        let index = start_index + offset;
        let resolved_step = TaskStep {
            tool: step.tool.clone(),
            args: resolve_turn_args(&step.args, &turns),
        };
        persist_task_runtime_state(
            config,
            &TaskRuntimeState {
                task: task.clone(),
                request: request_to_value(request)?,
                current_step_index: index,
                remaining_steps: remaining_steps[offset..].to_vec(),
                tool_turns: tool_turns_to_values(&turns),
                replans_remaining,
                status: task.status.clone(),
            },
        )?;
        let allowed = request
            .available_tools
            .iter()
            .any(|tool| tool.name == resolved_step.tool);
        if !allowed {
            return Ok(finalize_action(
                progress,
                DaemonAction::Ask {
                    plugin: request.plugin.clone(),
                    question: format!(
                        "The plan requested an unavailable tool: {}",
                        resolved_step.tool
                    ),
                },
                streaming,
            ));
        }
        if let Some(action) = maybe_request_permission(
            config,
            request,
            &turns,
            resolved_step.clone(),
            PendingContinuation::Plan {
                task: task.clone(),
                current_step_index: index,
                remaining_steps: remaining_steps[offset..].to_vec(),
                replans_remaining,
            },
        )? {
            if matches!(action, DaemonAction::PermissionRequest { .. }) {
                task.status = "awaiting_permission".into();
                persist_task_record(config, task)?;
                persist_task_runtime_state(
                    config,
                    &TaskRuntimeState {
                        task: task.clone(),
                        request: request_to_value(request)?,
                        current_step_index: index,
                        remaining_steps: remaining_steps[offset..].to_vec(),
                        tool_turns: tool_turns_to_values(&turns),
                        replans_remaining,
                        status: task.status.clone(),
                    },
                )?;
            }
            return Ok(finalize_action(progress, action, streaming));
        }
        if let Some(reason) =
            recursive_self_invocation_reason(request, &resolved_step.tool, &resolved_step.args)
        {
            task.mark_step_failed(index, &reason);
            persist_task_record(config, task)?;
            let failed = DaemonAction::TaskStep {
                plugin: request.plugin.clone(),
                task_id: task.id.clone(),
                index: index + 1,
                total: task.steps.len(),
                tool: resolved_step.tool.clone(),
                status: "failed".into(),
                summary: reason.clone(),
            };
            emit_action(emitter, &failed, streaming)?;
            progress.push(failed);
            return replan_after_step_failure(
                config,
                request,
                task,
                index,
                &resolved_step,
                &turns,
                progress,
                replans_remaining,
                streaming,
                emitter,
                &reason,
                model_output,
            );
        }

        if is_interactive_shell_tool(&resolved_step.tool) {
            if let Some(action) = emit_tool_running(
                emitter,
                request,
                &resolved_step.tool,
                &resolved_step.args,
                streaming,
            )? {
                progress.push(action);
            }
        } else {
            let running = DaemonAction::TaskStep {
                plugin: request.plugin.clone(),
                task_id: task.id.clone(),
                index: index + 1,
                total: task.steps.len(),
                tool: resolved_step.tool.clone(),
                status: "running".into(),
                summary: tool_running_summary(&resolved_step.tool, &resolved_step.args),
            };
            emit_action(emitter, &running, streaming)?;
            progress.push(running);
        }
        task.mark_running(index);
        persist_task_record(config, task)?;

        let normalized_args = prepare_builtin_tool_args(
            &resolved_step.tool,
            &resolved_step.args,
            Some(&request.working_directory),
        );
        let result = match invoke_builtin_tool(
            config,
            Some(&request.working_directory),
            &resolved_step.tool,
            &resolved_step.args,
        ) {
            Ok(result) => result,
            Err(error) => {
                task.mark_step_failed(index, &error);
                persist_task_record(config, task)?;
                let failed = DaemonAction::TaskStep {
                    plugin: request.plugin.clone(),
                    task_id: task.id.clone(),
                    index: index + 1,
                    total: task.steps.len(),
                    tool: resolved_step.tool.clone(),
                    status: "failed".into(),
                    summary: error.clone(),
                };
                emit_action(emitter, &failed, streaming)?;
                progress.push(failed);
                return replan_after_step_failure(
                    config,
                    request,
                    task,
                    index,
                    &resolved_step,
                    &turns,
                    progress,
                    replans_remaining,
                    streaming,
                    emitter,
                    &error,
                    model_output,
                );
            }
        };

        task.mark_step_finished(index, &result);
        persist_task_record(config, task)?;
        if is_interactive_shell_tool(&resolved_step.tool) {
            if let Some(action) = emit_tool_done(
                emitter,
                request,
                &resolved_step.tool,
                &resolved_step.args,
                &result,
                streaming,
            )? {
                progress.push(action);
            }
        } else {
            let done = DaemonAction::TaskStep {
                plugin: request.plugin.clone(),
                task_id: task.id.clone(),
                index: index + 1,
                total: task.steps.len(),
                tool: resolved_step.tool.clone(),
                status: "done".into(),
                summary: tool_done_summary(&resolved_step.tool, &resolved_step.args, &result),
            };
            emit_action(emitter, &done, streaming)?;
            progress.push(done);
        }
        turns.push(ToolTurn {
            tool: resolved_step.tool.clone(),
            args: normalized_args,
            result,
        });
        if interactive_session_needs_observation(&turns) {
            let _ = maybe_autoread_active_interactive_session(
                config,
                request,
                &mut turns,
                &mut progress,
                streaming,
                emitter,
                true,
            )?;
        }
    }
    if let Some(active) = active_interactive_session(&turns) {
        if active.needs_observation {
            let autoread = maybe_autoread_active_interactive_session(
                config,
                request,
                &mut turns,
                &mut progress,
                streaming,
                emitter,
                false,
            )?;
            if autoread.progressed {
                return execute_plan_remaining(
                    config,
                    request,
                    task,
                    task.steps.len(),
                    Vec::new(),
                    turns,
                    progress,
                    replans_remaining,
                    streaming,
                    emitter,
                    model_output,
                );
            }
        }
    }
    if interactive_prompt_pending(&turns) {
        match request_interactive_continuation(
            request,
            &turns,
            Some(task.summary.as_str()),
            model_output,
        )? {
            PlannedChatStep::Tool(step) => {
                let next_index = task.steps.len();
                task.status = "running".into();
                task.replace_remaining_steps(next_index, std::slice::from_ref(&step));
                persist_task_record(config, task)?;
                persist_task_runtime_state(
                    config,
                    &TaskRuntimeState {
                        task: task.clone(),
                        request: request_to_value(request)?,
                        current_step_index: next_index,
                        remaining_steps: vec![step.clone()],
                        tool_turns: tool_turns_to_values(&turns),
                        replans_remaining,
                        status: task.status.clone(),
                    },
                )?;
                return execute_plan_remaining(
                    config,
                    request,
                    task,
                    next_index,
                    vec![step],
                    turns,
                    progress,
                    replans_remaining,
                    streaming,
                    emitter,
                    model_output,
                );
            }
            PlannedChatStep::Plan(plan) => {
                let next_index = task.steps.len();
                task.status = "running".into();
                if !plan.summary.trim().is_empty() {
                    task.summary = plan.summary.clone();
                }
                task.replace_remaining_steps(next_index, &plan.steps);
                persist_task_record(config, task)?;
                persist_task_runtime_state(
                    config,
                    &TaskRuntimeState {
                        task: task.clone(),
                        request: request_to_value(request)?,
                        current_step_index: next_index,
                        remaining_steps: plan.steps.clone(),
                        tool_turns: tool_turns_to_values(&turns),
                        replans_remaining,
                        status: task.status.clone(),
                    },
                )?;
                return execute_plan_remaining(
                    config,
                    request,
                    task,
                    next_index,
                    plan.steps,
                    turns,
                    progress,
                    replans_remaining,
                    streaming,
                    emitter,
                    model_output,
                );
            }
            PlannedChatStep::Final(_) => {
                return Ok(finalize_action(
                    progress,
                    DaemonAction::Ask {
                        plugin: request.plugin.clone(),
                        question: "The interactive command is waiting for input and I need to continue it before answering you.".into(),
                    },
                    streaming,
                ));
            }
        }
    }
    let turn_contexts = tool_turn_contexts(&turns);
    let final_prompt = build_chat_execution_prompt(
        &request.base_prompt,
        &request.memory_context,
        &request.available_tools,
        request.include_tool_context,
        request.tool_calling_enabled,
        &turn_contexts,
        Some(TaskDirectiveContext::Finalize {
            summary: task.summary.clone(),
        }),
    );
    let response = model_output(&final_prompt)?;
    match parse_planned_chat_step(&response) {
        PlannedChatStep::Final(text) => {
            if interactive_prompt_pending(&turns) {
                match request_interactive_continuation(
                    request,
                    &turns,
                    Some(task.summary.as_str()),
                    model_output,
                )? {
                    PlannedChatStep::Tool(step) => {
                        let next_index = task.steps.len();
                        task.status = "running".into();
                        task.replace_remaining_steps(next_index, std::slice::from_ref(&step));
                        persist_task_record(config, task)?;
                        persist_task_runtime_state(
                            config,
                            &TaskRuntimeState {
                                task: task.clone(),
                                request: request_to_value(request)?,
                                current_step_index: next_index,
                                remaining_steps: vec![step.clone()],
                                tool_turns: tool_turns_to_values(&turns),
                                replans_remaining,
                                status: task.status.clone(),
                            },
                        )?;
                        return execute_plan_remaining(
                            config,
                            request,
                            task,
                            next_index,
                            vec![step],
                            turns,
                            progress,
                            replans_remaining,
                            streaming,
                            emitter,
                            model_output,
                        );
                    }
                    PlannedChatStep::Plan(plan) => {
                        let next_index = task.steps.len();
                        task.status = "running".into();
                        if !plan.summary.trim().is_empty() {
                            task.summary = plan.summary.clone();
                        }
                        task.replace_remaining_steps(next_index, &plan.steps);
                        persist_task_record(config, task)?;
                        persist_task_runtime_state(
                            config,
                            &TaskRuntimeState {
                                task: task.clone(),
                                request: request_to_value(request)?,
                                current_step_index: next_index,
                                remaining_steps: plan.steps.clone(),
                                tool_turns: tool_turns_to_values(&turns),
                                replans_remaining,
                                status: task.status.clone(),
                            },
                        )?;
                        return execute_plan_remaining(
                            config,
                            request,
                            task,
                            next_index,
                            plan.steps,
                            turns,
                            progress,
                            replans_remaining,
                            streaming,
                            emitter,
                            model_output,
                        );
                    }
                    PlannedChatStep::Final(_) => {
                        return Ok(finalize_action(
                            progress,
                            DaemonAction::Ask {
                                plugin: request.plugin.clone(),
                                question: "The interactive command is waiting for input and I need to continue it before answering you.".into(),
                            },
                            streaming,
                        ));
                    }
                }
            }
            task.mark_completed();
            persist_task_record(config, task)?;
            clear_task_runtime_state(config, &task.id)?;
            let finished = DaemonAction::TaskFinished {
                plugin: request.plugin.clone(),
                task_id: task.id.clone(),
                status: task.status.clone(),
                summary: task.summary.clone(),
            };
            emit_action(emitter, &finished, streaming)?;
            progress.push(finished);
            maybe_close_active_interactive_session(config, request, &mut turns)?;
            Ok(finalize_action(
                progress,
                DaemonAction::Message {
                    plugin: request.plugin.clone(),
                    message: text,
                },
                streaming,
            ))
        }
        PlannedChatStep::Tool(step) => {
            let next_index = task.steps.len();
            task.status = "running".into();
            task.replace_remaining_steps(next_index, std::slice::from_ref(&step));
            persist_task_record(config, task)?;
            persist_task_runtime_state(
                config,
                &TaskRuntimeState {
                    task: task.clone(),
                    request: request_to_value(request)?,
                    current_step_index: next_index,
                    remaining_steps: vec![step.clone()],
                    tool_turns: tool_turns_to_values(&turns),
                    replans_remaining,
                    status: task.status.clone(),
                },
            )?;
            execute_plan_remaining(
                config,
                request,
                task,
                next_index,
                vec![step],
                turns,
                progress,
                replans_remaining,
                streaming,
                emitter,
                model_output,
            )
        }
        PlannedChatStep::Plan(plan) => {
            let next_index = task.steps.len();
            task.status = "running".into();
            if !plan.summary.trim().is_empty() {
                task.summary = plan.summary.clone();
            }
            task.replace_remaining_steps(next_index, &plan.steps);
            persist_task_record(config, task)?;
            persist_task_runtime_state(
                config,
                &TaskRuntimeState {
                    task: task.clone(),
                    request: request_to_value(request)?,
                    current_step_index: next_index,
                    remaining_steps: plan.steps.clone(),
                    tool_turns: tool_turns_to_values(&turns),
                    replans_remaining,
                    status: task.status.clone(),
                },
            )?;
            execute_plan_remaining(
                config,
                request,
                task,
                next_index,
                plan.steps,
                turns,
                progress,
                replans_remaining,
                streaming,
                emitter,
                model_output,
            )
        }
    }
}

fn resolve_turn_args(value: &Value, turns: &[ToolTurn]) -> Value {
    match value {
        Value::String(text) => resolve_turn_reference(text, turns).unwrap_or_else(|| value.clone()),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_turn_args(item, turns))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), resolve_turn_args(value, turns)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn resolve_turn_reference(text: &str, turns: &[ToolTurn]) -> Option<Value> {
    let rest = text.strip_prefix("__TOOL_RESULT_")?;
    let (index, path) = rest.split_once("__.")?;
    let index = index.parse::<usize>().ok()?;
    let turn = turns.get(index)?;
    let value = serde_json::to_value(&turn.result).ok()?;
    let mut current = &value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current.clone())
}

fn replan_after_step_failure(
    config: &Value,
    request: &ChatExecutionConfig,
    task: &mut TaskRecord,
    failed_index: usize,
    failed_step: &TaskStep,
    turns: &[ToolTurn],
    mut progress: Vec<DaemonAction>,
    replans_remaining: usize,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    error: &str,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<DaemonAction, String> {
    if replans_remaining == 0 {
        task.mark_failed(error);
        persist_task_record(config, task)?;
        persist_task_runtime_state(
            config,
            &TaskRuntimeState {
                task: task.clone(),
                request: request_to_value(request)?,
                current_step_index: failed_index,
                remaining_steps: vec![failed_step.clone()],
                tool_turns: tool_turns_to_values(turns),
                replans_remaining,
                status: task.status.clone(),
            },
        )?;
        let finished = DaemonAction::TaskFinished {
            plugin: request.plugin.clone(),
            task_id: task.id.clone(),
            status: task.status.clone(),
            summary: task.summary.clone(),
        };
        emit_action(emitter, &finished, streaming)?;
        progress.push(finished);
        return Ok(finalize_action(
            progress,
            DaemonAction::Ask {
                plugin: request.plugin.clone(),
                question: format!("Task failed after {}: {}", failed_step.tool, error),
            },
            streaming,
        ));
    }

    let turn_contexts = tool_turn_contexts(turns);
    let prompt = build_chat_execution_prompt(
        &request.base_prompt,
        &request.memory_context,
        &request.available_tools,
        request.include_tool_context,
        request.tool_calling_enabled,
        &turn_contexts,
        Some(TaskDirectiveContext::Replan {
            summary: task.summary.clone(),
            goal: task.goal.clone(),
            failed_step_tool: failed_step.tool.clone(),
            failed_step_args: failed_step.args.clone(),
            error: error.to_string(),
        }),
    );
    let response = model_output(&prompt)?;
    match parse_planned_chat_step(&response) {
        PlannedChatStep::Final(text) => {
            task.mark_completed();
            persist_task_record(config, task)?;
            clear_task_runtime_state(config, &task.id)?;
            let finished = DaemonAction::TaskFinished {
                plugin: request.plugin.clone(),
                task_id: task.id.clone(),
                status: task.status.clone(),
                summary: task.summary.clone(),
            };
            emit_action(emitter, &finished, streaming)?;
            progress.push(finished);
            Ok(finalize_action(
                progress,
                DaemonAction::Message {
                    plugin: request.plugin.clone(),
                    message: text,
                },
                streaming,
            ))
        }
        PlannedChatStep::Tool(step) => {
            let plan = TaskPlan {
                summary: format!("replan after {}", failed_step.tool),
                steps: vec![step],
            };
            task.replace_remaining_steps(failed_index + 1, &plan.steps);
            task.summary = plan.summary.clone();
            persist_task_record(config, task)?;
            persist_task_runtime_state(
                config,
                &TaskRuntimeState {
                    task: task.clone(),
                    request: request_to_value(request)?,
                    current_step_index: failed_index + 1,
                    remaining_steps: plan.steps.clone(),
                    tool_turns: tool_turns_to_values(turns),
                    replans_remaining: replans_remaining.saturating_sub(1),
                    status: task.status.clone(),
                },
            )?;
            let replanning = DaemonAction::TaskStep {
                plugin: request.plugin.clone(),
                task_id: task.id.clone(),
                index: failed_index + 1,
                total: task.steps.len(),
                tool: failed_step.tool.clone(),
                status: "replanning".into(),
                summary: task.summary.clone(),
            };
            emit_action(emitter, &replanning, streaming)?;
            progress.push(replanning);
            execute_plan_remaining(
                config,
                request,
                task,
                failed_index + 1,
                plan.steps,
                turns.to_vec(),
                progress,
                replans_remaining.saturating_sub(1),
                streaming,
                emitter,
                model_output,
            )
        }
        PlannedChatStep::Plan(plan) => {
            task.summary = if plan.summary.trim().is_empty() {
                task.summary.clone()
            } else {
                plan.summary.clone()
            };
            task.replace_remaining_steps(failed_index + 1, &plan.steps);
            persist_task_record(config, task)?;
            persist_task_runtime_state(
                config,
                &TaskRuntimeState {
                    task: task.clone(),
                    request: request_to_value(request)?,
                    current_step_index: failed_index + 1,
                    remaining_steps: plan.steps.clone(),
                    tool_turns: tool_turns_to_values(turns),
                    replans_remaining: replans_remaining.saturating_sub(1),
                    status: task.status.clone(),
                },
            )?;
            let replanning = DaemonAction::TaskStep {
                plugin: request.plugin.clone(),
                task_id: task.id.clone(),
                index: failed_index + 1,
                total: task.steps.len(),
                tool: failed_step.tool.clone(),
                status: "replanning".into(),
                summary: task.summary.clone(),
            };
            emit_action(emitter, &replanning, streaming)?;
            progress.push(replanning);
            execute_plan_remaining(
                config,
                request,
                task,
                failed_index + 1,
                plan.steps,
                turns.to_vec(),
                progress,
                replans_remaining.saturating_sub(1),
                streaming,
                emitter,
                model_output,
            )
        }
    }
}

fn maybe_request_permission(
    config: &Value,
    request: &ChatExecutionConfig,
    turns: &[ToolTurn],
    pending_step: TaskStep,
    continuation: PendingContinuation,
) -> Result<Option<DaemonAction>, String> {
    match permission_decision_for_tool(config, &pending_step.tool) {
        ToolPermissionDecision::Allow => Ok(None),
        ToolPermissionDecision::Deny => Ok(Some(DaemonAction::Ask {
            plugin: request.plugin.clone(),
            question: format!("Permission policy denies {}.", pending_step.tool),
        })),
        ToolPermissionDecision::Ask => {
            let permission_id = format!("perm-{}", unix_now());
            let permission_class = tool_definition_by_name(&pending_step.tool)
                .map(|tool| tool.permission.as_str().to_string())
                .unwrap_or_else(|| "external".into());
            let pending = PendingPermissionRequest {
                id: permission_id.clone(),
                plugin: request.plugin.clone(),
                tool: pending_step.tool.clone(),
                permission_class: permission_class.clone(),
                summary: if permission_class == "interactive_shell" {
                    "Allow interactive shell?".into()
                } else {
                    format!("Allow {} to use {}?", request.plugin, pending_step.tool)
                },
                request: request_to_snapshot(request),
                tool_turns: turns.iter().map(tool_turn_to_snapshot).collect(),
                pending_step,
                continuation,
            };
            persist_pending_permission(config, &pending)?;
            Ok(Some(DaemonAction::PermissionRequest {
                plugin: request.plugin.clone(),
                permission_id,
                tool: pending.tool,
                permission_class,
                summary: pending.summary,
            }))
        }
    }
}

fn batch_action(mut progress: Vec<DaemonAction>, final_action: DaemonAction) -> DaemonAction {
    if progress.is_empty() {
        return final_action;
    }
    let plugin = match &final_action {
        DaemonAction::Message { plugin, .. }
        | DaemonAction::ReloadRuntime { plugin, .. }
        | DaemonAction::Ask { plugin, .. }
        | DaemonAction::Run { plugin, .. }
        | DaemonAction::Select { plugin, .. }
        | DaemonAction::PermissionRequest { plugin, .. }
        | DaemonAction::ToolStep { plugin, .. }
        | DaemonAction::SessionStarted { plugin, .. }
        | DaemonAction::SessionInput { plugin, .. }
        | DaemonAction::SessionOutput { plugin, .. }
        | DaemonAction::SessionClosed { plugin, .. }
        | DaemonAction::TaskStarted { plugin, .. }
        | DaemonAction::TaskStep { plugin, .. }
        | DaemonAction::TaskFinished { plugin, .. }
        | DaemonAction::Batch { plugin, .. }
        | DaemonAction::Noop { plugin, .. } => plugin.clone(),
    };
    progress.push(final_action);
    DaemonAction::Batch {
        plugin,
        actions: progress,
    }
}

fn finalize_action(
    progress: Vec<DaemonAction>,
    final_action: DaemonAction,
    streaming: bool,
) -> DaemonAction {
    if streaming {
        final_action
    } else {
        batch_action(progress, final_action)
    }
}

fn emit_action(
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    action: &DaemonAction,
    streaming: bool,
) -> Result<(), String> {
    if streaming {
        emitter(action)?;
    }
    Ok(())
}

fn emit_tool_running(
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    request: &ChatExecutionConfig,
    tool: &str,
    args: &Value,
    streaming: bool,
) -> Result<Option<DaemonAction>, String> {
    let action = match tool {
        "interactive_shell_start" => Some(DaemonAction::SessionStarted {
            plugin: request.plugin.clone(),
            command: args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("interactive shell")
                .to_string(),
        }),
        "interactive_shell_write" => Some(DaemonAction::SessionInput {
            plugin: request.plugin.clone(),
            text: render_interactive_shell_input(args),
        }),
        "interactive_shell_key" => Some(DaemonAction::SessionInput {
            plugin: request.plugin.clone(),
            text: render_interactive_shell_key(args),
        }),
        "interactive_shell_read" | "interactive_shell_close" => None,
        _ => Some(DaemonAction::ToolStep {
            plugin: request.plugin.clone(),
            tool: tool.to_string(),
            status: "running".into(),
            summary: tool_running_summary(tool, args),
        }),
    };
    if let Some(action) = &action {
        emit_action(emitter, action, streaming)?;
    }
    Ok(action)
}

fn emit_tool_done(
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    request: &ChatExecutionConfig,
    tool: &str,
    args: &Value,
    result: &ToolCallResult,
    streaming: bool,
) -> Result<Option<DaemonAction>, String> {
    let action = match tool {
        "interactive_shell_start" => None,
        "interactive_shell_write" => None,
        "interactive_shell_key" => None,
        "interactive_shell_read" => {
            let text = result
                .output
                .get("display_output")
                .and_then(Value::as_str)
                .or_else(|| result.output.get("output").and_then(Value::as_str))
                .or_else(|| result.output.get("screen_text").and_then(Value::as_str))
                .or_else(|| result.output.get("screen_tail").and_then(Value::as_str))
                .unwrap_or("");
            if text.is_empty() {
                None
            } else {
                Some(DaemonAction::SessionOutput {
                    plugin: request.plugin.clone(),
                    text: text.to_string(),
                })
            }
        }
        "interactive_shell_close" => Some(DaemonAction::SessionClosed {
            plugin: request.plugin.clone(),
        }),
        _ => Some(DaemonAction::ToolStep {
            plugin: request.plugin.clone(),
            tool: tool.to_string(),
            status: "done".into(),
            summary: tool_done_summary(tool, args, result),
        }),
    };
    if let Some(action) = &action {
        emit_action(emitter, action, streaming)?;
    }
    Ok(action)
}

fn is_interactive_shell_tool(tool: &str) -> bool {
    matches!(
        tool,
        "interactive_shell_start"
            | "interactive_shell_read"
            | "interactive_shell_write"
            | "interactive_shell_key"
            | "interactive_shell_close"
    )
}

#[derive(Debug, Clone)]
struct ActiveInteractiveSession {
    session_id: String,
    since_seq: u64,
    needs_observation: bool,
}

fn interactive_prompt_pending(turns: &[ToolTurn]) -> bool {
    active_interactive_session(turns)
        .map(|session| !session.needs_observation)
        .unwrap_or(false)
}

fn request_interactive_continuation(
    request: &ChatExecutionConfig,
    turns: &[ToolTurn],
    summary: Option<&str>,
    model_output: &dyn Fn(&str) -> Result<String, String>,
) -> Result<PlannedChatStep, String> {
    let turn_contexts = tool_turn_contexts(turns);
    let prompt = build_chat_execution_prompt(
        &request.base_prompt,
        &request.memory_context,
        &request.available_tools,
        request.include_tool_context,
        request.tool_calling_enabled,
        &turn_contexts,
        Some(TaskDirectiveContext::ContinueInteractive {
            summary: summary.unwrap_or("").to_string(),
        }),
    );
    let response = model_output(&prompt)?;
    Ok(parse_planned_chat_step(&response))
}

fn execute_tool_step_inline(
    config: &Value,
    request: &ChatExecutionConfig,
    turns: &mut Vec<ToolTurn>,
    progress: &mut Vec<DaemonAction>,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    tool: &str,
    args: &Value,
) -> Result<(), String> {
    if let Some(action) = emit_tool_running(emitter, request, tool, args, streaming)? {
        progress.push(action);
    }
    let normalized_args = prepare_builtin_tool_args(tool, args, Some(&request.working_directory));
    let result = invoke_builtin_tool(config, Some(&request.working_directory), tool, args)?;
    if let Some(action) =
        emit_tool_done(emitter, request, tool, &normalized_args, &result, streaming)?
    {
        progress.push(action);
    }
    turns.push(ToolTurn {
        tool: tool.to_string(),
        args: normalized_args,
        result,
    });
    if interactive_session_needs_observation(turns) {
        let _ = maybe_autoread_active_interactive_session(
            config, request, turns, progress, streaming, emitter, true,
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default)]
struct AutoReadOutcome {
    progressed: bool,
    still_running: bool,
}

fn active_interactive_session(turns: &[ToolTurn]) -> Option<ActiveInteractiveSession> {
    let mut active: Option<ActiveInteractiveSession> = None;
    for turn in turns {
        match turn.tool.as_str() {
            "interactive_shell_start" => {
                let session_id = turn
                    .result
                    .output
                    .get("session_id")
                    .and_then(Value::as_str)?
                    .to_string();
                active = Some(ActiveInteractiveSession {
                    session_id,
                    since_seq: 0,
                    needs_observation: true,
                });
            }
            "interactive_shell_write" => {
                let session_id = turn
                    .args
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(current) = active.as_mut() {
                    if current.session_id == session_id {
                        current.needs_observation = true;
                    }
                }
            }
            "interactive_shell_key" => {
                let session_id = turn
                    .args
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(current) = active.as_mut() {
                    if current.session_id == session_id {
                        current.needs_observation = true;
                    }
                }
            }
            "interactive_shell_read" => {
                let session_id = turn
                    .result
                    .output
                    .get("session_id")
                    .and_then(Value::as_str)?
                    .to_string();
                let closed = turn
                    .result
                    .output
                    .get("closed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if closed {
                    active = None;
                } else {
                    active = Some(ActiveInteractiveSession {
                        session_id,
                        since_seq: turn
                            .result
                            .output
                            .get("end_seq")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        needs_observation: false,
                    });
                }
            }
            "interactive_shell_close" => active = None,
            _ => {}
        }
    }
    active
}

fn interactive_session_needs_observation(turns: &[ToolTurn]) -> bool {
    active_interactive_session(turns)
        .map(|session| session.needs_observation)
        .unwrap_or(false)
}

fn maybe_autoread_active_interactive_session(
    config: &Value,
    request: &ChatExecutionConfig,
    turns: &mut Vec<ToolTurn>,
    progress: &mut Vec<DaemonAction>,
    streaming: bool,
    emitter: &mut dyn FnMut(&DaemonAction) -> Result<(), String>,
    require_pending_observation: bool,
) -> Result<AutoReadOutcome, String> {
    let Some(active) = active_interactive_session(turns) else {
        return Ok(AutoReadOutcome::default());
    };
    if require_pending_observation && !active.needs_observation {
        return Ok(AutoReadOutcome::default());
    }
    let mut active = active;
    let mut attempts = 0;
    loop {
        let args = json!({
            "session_id": active.session_id,
            "since_seq": active.since_seq,
            "wait_ms": 4000,
            "settle_ms": 1000,
        });
        let result = invoke_builtin_tool(
            config,
            Some(&request.working_directory),
            "interactive_shell_read",
            &args,
        )?;
        let had_output = result
            .output
            .get("output")
            .and_then(Value::as_str)
            .map(|value| !value.is_empty())
            .unwrap_or(false);
        let closed = result
            .output
            .get("closed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let normalized_args = prepare_builtin_tool_args(
            "interactive_shell_read",
            &args,
            Some(&request.working_directory),
        );
        if let Some(action) = emit_tool_done(
            emitter,
            request,
            "interactive_shell_read",
            &normalized_args,
            &result,
            streaming,
        )? {
            progress.push(action);
        }
        active.since_seq = result
            .output
            .get("end_seq")
            .and_then(Value::as_u64)
            .unwrap_or(active.since_seq);
        turns.push(ToolTurn {
            tool: "interactive_shell_read".into(),
            args: normalized_args,
            result,
        });
        if closed || had_output {
            return Ok(AutoReadOutcome {
                progressed: true,
                still_running: !closed,
            });
        }
        attempts += 1;
        if attempts >= 4 {
            return Ok(AutoReadOutcome {
                progressed: false,
                still_running: true,
            });
        }
    }
}

fn maybe_close_active_interactive_session(
    config: &Value,
    request: &ChatExecutionConfig,
    turns: &mut Vec<ToolTurn>,
) -> Result<(), String> {
    let Some(active) = active_interactive_session(turns) else {
        return Ok(());
    };
    let args = json!({
        "session_id": active.session_id,
    });
    let result = invoke_builtin_tool(
        config,
        Some(&request.working_directory),
        "interactive_shell_close",
        &args,
    )?;
    let normalized_args = prepare_builtin_tool_args(
        "interactive_shell_close",
        &args,
        Some(&request.working_directory),
    );
    turns.push(ToolTurn {
        tool: "interactive_shell_close".into(),
        args: normalized_args,
        result,
    });
    Ok(())
}

fn recursive_self_invocation_reason(
    request: &ChatExecutionConfig,
    tool: &str,
    args: &Value,
) -> Option<String> {
    if request_explicitly_targets_noodle(&request.input) {
        return None;
    }
    let command = match tool {
        "shell_exec" | "interactive_shell_start" => args.get("command").and_then(Value::as_str)?,
        _ => return None,
    };
    if command_invokes_noodle(command) {
        Some(
            "Refused recursive noodle invocation. Invoke the target command directly instead of routing back through oo, ww, or noodle."
                .into(),
        )
    } else {
        None
    }
}

fn command_invokes_noodle(command: &str) -> bool {
    let normalized = command
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-') {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>();
    normalized.split_whitespace().any(|token| {
        matches!(token, "oo" | "ww" | "noodle")
            || token.ends_with("/noodle")
            || token.ends_with("/bin/noodle")
    })
}

fn tool_running_summary(tool: &str, args: &Value) -> String {
    match tool {
        "file_read" => format!(
            "Reading {}",
            compact_path(args.get("path").and_then(Value::as_str).unwrap_or("file"))
        ),
        "path_search" => format!(
            "Searching for {}",
            compact_text(args.get("query").and_then(Value::as_str).unwrap_or("path"))
        ),
        "glob" => format!(
            "Scanning {} for {}",
            compact_path(args.get("root").and_then(Value::as_str).unwrap_or(".")),
            compact_text(args.get("pattern").and_then(Value::as_str).unwrap_or("*"))
        ),
        "grep" => format!(
            "Searching {} for {}",
            compact_path(args.get("root").and_then(Value::as_str).unwrap_or(".")),
            compact_text(
                args.get("pattern")
                    .and_then(Value::as_str)
                    .unwrap_or("text")
            )
        ),
        "web_fetch" => format!(
            "Fetching {}",
            compact_text(args.get("url").and_then(Value::as_str).unwrap_or("url"))
        ),
        "web_search" => format!(
            "Searching the web for {}",
            compact_text(args.get("query").and_then(Value::as_str).unwrap_or("query"))
        ),
        "file_write" => format!(
            "Writing {}",
            compact_path(args.get("path").and_then(Value::as_str).unwrap_or("file"))
        ),
        "file_edit" => format!(
            "Editing {}",
            compact_path(args.get("path").and_then(Value::as_str).unwrap_or("file"))
        ),
        "shell_exec" => format!(
            "Running {}",
            compact_text(
                args.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("command")
            )
        ),
        "interactive_shell_start" => format!(
            "Starting interactive shell for {}",
            compact_text(
                args.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("command")
            )
        ),
        "interactive_shell_read" => "Reading interactive shell output".into(),
        "interactive_shell_write" => format!(
            "Sending input: {}",
            compact_text(&render_interactive_shell_input(args))
        ),
        "interactive_shell_key" => format!(
            "Pressing key: {}",
            compact_text(&render_interactive_shell_key(args))
        ),
        "interactive_shell_close" => "Closing interactive shell".into(),
        "memory_query" => "Querying noodle memory".into(),
        "mcp_resource_read" => "Reading MCP resource".into(),
        "task_note_write" => "Writing task note".into(),
        "agent_handoff_create" => "Creating agent handoff".into(),
        _ => format!("Running {}", tool),
    }
}

fn tool_done_summary(tool: &str, args: &Value, result: &ToolCallResult) -> String {
    let output = &result.output;
    match tool {
        "file_read" => {
            let path = compact_path(args.get("path").and_then(Value::as_str).unwrap_or("file"));
            let bytes = output
                .get("bytes")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Read {} ({} bytes)", path, bytes)
        }
        "path_search" => {
            let matches = output
                .get("matches")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            match matches.first().and_then(Value::as_str) {
                Some(first) => format!(
                    "Found {} match(es); first: {}",
                    matches.len(),
                    compact_path(first)
                ),
                None => "No matches found".into(),
            }
        }
        "glob" => {
            let matches = output
                .get("matches")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            match matches.first().and_then(Value::as_str) {
                Some(first) => format!(
                    "Matched {} path(s); first: {}",
                    matches.len(),
                    compact_path(first)
                ),
                None => "No matching paths".into(),
            }
        }
        "grep" => {
            let matches = output
                .get("matches")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            match matches.first() {
                Some(first) => format!(
                    "Found {} match(es); first: {}:{} {}",
                    matches.len(),
                    compact_path(first.get("path").and_then(Value::as_str).unwrap_or("file")),
                    first
                        .get("line")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    compact_text(first.get("text").and_then(Value::as_str).unwrap_or(""))
                ),
                None => "No text matches".into(),
            }
        }
        "web_fetch" => format!(
            "Fetched {}",
            compact_text(output.get("url").and_then(Value::as_str).unwrap_or("url"))
        ),
        "web_search" => {
            let results = output
                .get("results")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            format!("Search returned {} result(s)", results)
        }
        "file_write" => format!(
            "Wrote {}",
            compact_path(args.get("path").and_then(Value::as_str).unwrap_or("file"))
        ),
        "file_edit" => format!(
            "Edited {}",
            compact_path(args.get("path").and_then(Value::as_str).unwrap_or("file"))
        ),
        "shell_exec" => {
            let stdout = output.get("stdout").and_then(Value::as_str).unwrap_or("");
            if stdout.trim().is_empty() {
                "Command completed".into()
            } else {
                format!("Output: {}", compact_text(stdout))
            }
        }
        "interactive_shell_start" => "Interactive shell ready".into(),
        "interactive_shell_read" => {
            let text = output.get("output").and_then(Value::as_str).unwrap_or("");
            if text.trim().is_empty() {
                "No new output yet".into()
            } else {
                format!("Output: {}", compact_text(text))
            }
        }
        "interactive_shell_write" => format!(
            "Sent: {}",
            compact_text(&render_interactive_shell_input(args))
        ),
        "interactive_shell_key" => format!(
            "Pressed: {}",
            compact_text(&render_interactive_shell_key(args))
        ),
        "interactive_shell_close" => "Interactive shell closed".into(),
        "memory_query" => "Loaded noodle memory".into(),
        "mcp_resource_read" => "Loaded MCP resource".into(),
        "task_note_write" => "Task note written".into(),
        "agent_handoff_create" => "Agent handoff created".into(),
        _ => format!("Completed {}", tool),
    }
}

fn compact_text(value: &str) -> String {
    let cleaned = value.replace('\n', "\\n").replace('\r', "");
    let trimmed = cleaned.trim();
    if trimmed.len() <= 120 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..117])
    }
}

fn render_interactive_shell_input(args: &Value) -> String {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| args.get("input").and_then(Value::as_str))
        .unwrap_or("");
    let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
    match (text.is_empty(), submit) {
        (true, true) => "<Enter>".into(),
        (true, false) => String::new(),
        (false, true) => format!("{text}<Enter>"),
        (false, false) => text.into(),
    }
}

fn render_interactive_shell_key(args: &Value) -> String {
    let key = args.get("key").and_then(Value::as_str).unwrap_or("key");
    let repeat = args.get("repeat").and_then(Value::as_u64).unwrap_or(1);
    if repeat <= 1 {
        format!("<{}>", key)
    } else {
        format!("<{} x{}>", key, repeat)
    }
}

fn compact_path(value: &str) -> String {
    compact_text(value)
}

fn clean_text(text: &str) -> String {
    let mut cleaned = text.trim().replace("```json", "").replace("```", "");
    for pattern in ["<think>", "</think>", "<thinking>", "</thinking>"] {
        cleaned = cleaned.replace(pattern, "");
    }
    let cleaned = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let finals = cleaned
        .iter()
        .filter_map(|line| line.strip_prefix("FINAL:").map(str::trim))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if !finals.is_empty() {
        return finals.join("\n");
    }
    cleaned
        .into_iter()
        .filter(|line| {
            !line.starts_with("STEP:")
                && !line.starts_with("TOOL:")
                && !line.starts_with("PLAN:")
                && !line.starts_with("FINAL:")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_turn_contexts(turns: &[ToolTurn]) -> Vec<ToolTurnContext> {
    turns
        .iter()
        .map(|turn| ToolTurnContext {
            tool: turn.tool.clone(),
            args: turn.args.clone(),
            result: turn.result.output.clone(),
        })
        .collect()
}

fn request_to_value(request: &ChatExecutionConfig) -> Result<Value, String> {
    serde_json::to_value(request_to_snapshot(request)).map_err(|err| err.to_string())
}

fn request_from_value(value: &Value) -> Result<ChatExecutionConfig, String> {
    let snapshot = serde_json::from_value::<ChatExecutionSnapshot>(value.clone())
        .map_err(|err| err.to_string())?;
    Ok(snapshot_to_request(&snapshot))
}

fn tool_turns_to_values(turns: &[ToolTurn]) -> Vec<Value> {
    turns
        .iter()
        .map(|turn| {
            json!({
                "tool": turn.tool,
                "args": turn.args,
                "result": turn.result.output,
            })
        })
        .collect()
}

fn tool_turns_from_values(values: &[Value]) -> Result<Vec<ToolTurn>, String> {
    values
        .iter()
        .map(|value| {
            Ok(ToolTurn {
                tool: value
                    .get("tool")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing tool in runtime state".to_string())?
                    .to_string(),
                args: value.get("args").cloned().unwrap_or(Value::Null),
                result: ToolCallResult {
                    tool: value
                        .get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    ok: true,
                    output: value.get("result").cloned().unwrap_or(Value::Null),
                },
            })
        })
        .collect()
}

fn tool_turn_to_snapshot(turn: &ToolTurn) -> ToolTurnSnapshot {
    ToolTurnSnapshot {
        tool: turn.tool.clone(),
        args: turn.args.clone(),
        result: turn.result.output.clone(),
    }
}

fn snapshots_to_turns(items: &[ToolTurnSnapshot]) -> Vec<ToolTurn> {
    items
        .iter()
        .map(|item| ToolTurn {
            tool: item.tool.clone(),
            args: item.args.clone(),
            result: ToolCallResult {
                tool: item.tool.clone(),
                ok: true,
                output: item.result.clone(),
            },
        })
        .collect()
}

fn request_to_snapshot(request: &ChatExecutionConfig) -> ChatExecutionSnapshot {
    ChatExecutionSnapshot {
        plugin: request.plugin.clone(),
        input: request.input.clone(),
        working_directory: request.working_directory.clone(),
        base_prompt: request.base_prompt.clone(),
        memory_context: request.memory_context.clone(),
        include_tool_context: request.include_tool_context,
        tool_calling_enabled: request.tool_calling_enabled,
        task_execution_enabled: request.task_execution_enabled,
        max_tool_rounds: request.max_tool_rounds,
        max_replans: request.max_replans,
        available_tool_names: request
            .available_tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect(),
    }
}

fn snapshot_to_request(snapshot: &ChatExecutionSnapshot) -> ChatExecutionConfig {
    ChatExecutionConfig {
        plugin: snapshot.plugin.clone(),
        input: snapshot.input.clone(),
        working_directory: snapshot.working_directory.clone(),
        base_prompt: snapshot.base_prompt.clone(),
        memory_context: snapshot.memory_context.clone(),
        include_tool_context: snapshot.include_tool_context,
        tool_calling_enabled: snapshot.tool_calling_enabled,
        task_execution_enabled: snapshot.task_execution_enabled,
        max_tool_rounds: snapshot.max_tool_rounds,
        max_replans: snapshot.max_replans,
        available_tools: snapshot
            .available_tool_names
            .iter()
            .filter_map(|name| tool_definition_by_name(name))
            .collect(),
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
    use super::{ChatExecutionConfig, required_tool_use_reason, run_chat_execution};
    use crate::context_builder::build_chat_base_prompt;
    use crate::tooling::tool_definition_by_name;
    use serde_json::json;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn direct_local_write_requests_force_a_concrete_tool_step() {
        let working_directory = temp_dir("noodle-executor-write");
        let input = "create a alex.md file and write alexander in binary 12 times";
        let request = ChatExecutionConfig {
            plugin: "chat".into(),
            input: input.into(),
            working_directory: working_directory.display().to_string(),
            base_prompt: build_chat_base_prompt(
                "",
                input,
                &working_directory.display().to_string(),
                "zsh",
                "",
                None,
                &[],
            ),
            memory_context: String::new(),
            include_tool_context: false,
            tool_calling_enabled: true,
            task_execution_enabled: true,
            max_tool_rounds: 4,
            max_replans: 1,
            available_tools: vec![tool_definition_by_name("file_write").unwrap()],
        };
        assert!(required_tool_use_reason(&request).is_some());

        let responses = RefCell::new(VecDeque::from(vec![
            "FINAL: I can write that file.".to_string(),
            "FINAL: I need to inspect with tools before I can answer that confidently."
                .to_string(),
            r#"STEP: file_write {"path":"alex.md","content":"01100001\n01101100\n01100101\n01111000\n01100001\n01101110\n01100100\n01100101\n01110010\n01100001\n01101100\n01100101\n01111000\n01100001\n01101110\n01100100\n01100101\n01110010\n01100001\n01101100\n01100101\n01111000\n01100001\n01101110\n01100100\n01100101\n01110010\n01100001\n01101100\n01100101\n01111000\n01100001\n01101110\n01100100\n01100101\n01110010"}"#.to_string(),
            "FINAL: wrote alex.md".to_string(),
        ]));
        let prompts = RefCell::new(Vec::new());
        let config = json!({
            "permissions": {
                "classes": {
                    "local_write": "allow"
                }
            }
        });
        let mut noop = |_action: &crate::actions::DaemonAction| Ok(());
        let action = run_chat_execution(&config, request, false, &mut noop, &|prompt| {
            prompts.borrow_mut().push(prompt.to_string());
            responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "missing stub response".to_string())
        })
        .unwrap();

        let written = fs::read_to_string(working_directory.join("alex.md")).unwrap();
        assert!(written.contains("01100001"));
        assert_eq!(action.primary_text().as_deref(), Some("wrote alex.md"));
        let prompts = prompts.borrow();
        assert_eq!(prompts.len(), 4);
        assert!(
            prompts[1]
                .contains("This request explicitly requires tool use before any final answer")
        );
        assert!(prompts[2].contains("You must choose a tool-based next action now."));

        let _ = fs::remove_dir_all(&working_directory);
    }
}
