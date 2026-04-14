mod actions;
mod context_builder;
mod engine;
mod executor;
mod interactive_shell;
mod permissions;
mod planner;
mod tasks;
mod tooling;

use crate::tasks::{cancel_task, list_task_records, load_task_record, load_task_runtime_state};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use tooling::{
    PluginExecution, ToolDefinition, enabled_plugin_manifests, exported_mcp_tools,
    invoke_builtin_tool, plugin_matches_request, plugin_order, registered_slash_command_names,
    shutdown_all_mcp_sessions, tools_for_plugin,
};

const MODULE_API_VERSION: &str = "v1";

#[derive(Clone)]
struct CachedConfig {
    modified: Option<SystemTime>,
    value: Value,
}

#[derive(Debug, Clone)]
enum Command {
    Run {
        mode: String,
        input: String,
        cwd: String,
        shell: String,
        exit_status: i64,
        recent_command: String,
        selected_command: String,
        config: String,
        stream: bool,
    },
    RuntimeConfig {
        config: String,
    },
    ConfigValue {
        config: String,
        key: String,
        fallback: String,
        list: bool,
    },
    PayloadFields {
        payload: Option<String>,
    },
    ModuleApiInfo,
    ModelOutput {
        config: String,
        prompt: Option<String>,
        debug: bool,
    },
    ExecutionRun {
        config: String,
        request_json: Option<String>,
        debug: bool,
        stream: bool,
    },
    ExecutionResumePermission {
        config: String,
        permission_id: String,
        decision: String,
        debug: bool,
        stream: bool,
    },
    MemoryContext {
        config: String,
        plugin: String,
    },
    MemoryRecordTurns {
        config: String,
        plugin: String,
        user_text: String,
        payload_json: Option<String>,
        debug: bool,
    },
    WorkspaceContext {
        cwd: String,
    },
    ToolList {
        config: String,
        plugin: String,
    },
    ToolCall {
        config: String,
        tool: String,
        args_json: String,
    },
    ToolBatch {
        config: String,
        calls_json: String,
    },
    TaskList {
        config: String,
        status: String,
        limit: usize,
    },
    TaskShow {
        config: String,
        task_id: String,
    },
    TaskResume {
        config: String,
        task_id: String,
    },
    TaskCancel {
        config: String,
        task_id: String,
    },
    Mcp,
    Daemon {
        socket: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct DaemonRequest {
    command: String,
    mode: Option<String>,
    input: Option<String>,
    cwd: Option<String>,
    shell: Option<String>,
    exit_status: Option<i64>,
    recent_command: Option<String>,
    selected_command: Option<String>,
    config: Option<String>,
    key: Option<String>,
    fallback: Option<String>,
    list: Option<bool>,
    payload: Option<String>,
    plugin: Option<String>,
    tool: Option<String>,
    args_json: Option<String>,
    calls_json: Option<String>,
    task_id: Option<String>,
    status: Option<String>,
    limit: Option<usize>,
    stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DaemonResponse {
    ok: bool,
    output: String,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExternalPluginRequest<'a> {
    api_version: &'static str,
    event: &'a str,
    input: &'a str,
    cwd: &'a str,
    shell: &'a str,
    exit_status: i64,
    recent_command: &'a str,
    selected_command: &'a str,
    debug: bool,
    stream: bool,
    config_path: String,
    host: ExternalHostInfo,
    config: &'a Value,
}

#[derive(Debug, Serialize)]
struct ExternalHostInfo {
    binary_path: String,
    module_api: ExternalModuleApiInfo,
    module_order: Vec<String>,
    slash_commands: Vec<tooling::SlashCommandDefinition>,
    tool_counts: HashMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct ExternalModuleApiInfo {
    version: &'static str,
    command_prefix: Vec<String>,
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExternalPluginResponse {
    ok: bool,
    payload: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExternalPluginStreamEnvelope {
    #[serde(rename = "type")]
    kind: String,
    payload: Option<Value>,
    ok: Option<bool>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExternalExecutionRequestInput {
    plugin: String,
    input: String,
    working_directory: String,
    base_prompt: String,
    memory_context: String,
    include_tool_context: Option<bool>,
    tool_calling_enabled: Option<bool>,
    task_execution_enabled: Option<bool>,
    max_tool_rounds: Option<usize>,
    max_replans: Option<usize>,
}

#[derive(Debug, Clone)]
struct CompilePolicy {
    event_kind: &'static str,
    threshold: i64,
    artifact_kind: &'static str,
}

#[derive(Debug, Clone)]
struct PluginMemoryBehavior {
    event_limits: Vec<(&'static str, i64)>,
    compile: Option<CompilePolicy>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("noodle helper error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let command = parse_args()?;
    match command {
        Command::Mcp => serve_mcp_stdio(),
        Command::Daemon { socket } => serve_daemon(&socket),
        Command::Run { stream: true, .. }
        | Command::ExecutionRun { stream: true, .. }
        | Command::ExecutionResumePermission { stream: true, .. } => execute_streaming(command),
        Command::PayloadFields { .. }
        | Command::ModuleApiInfo
        | Command::ModelOutput { .. }
        | Command::MemoryContext { .. }
        | Command::MemoryRecordTurns { .. }
        | Command::WorkspaceContext { .. } => {
            let output = execute_local(command)?;
            print!("{output}");
            Ok(())
        }
        other => {
            let output = execute_via_daemon(other)?;
            print!("{output}");
            Ok(())
        }
    }
}

fn parse_args() -> Result<Command, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        return Err("missing arguments".into());
    }

    match args[0].as_str() {
        "mcp" => Ok(Command::Mcp),
        "daemon" => Ok(Command::Daemon {
            socket: optional_flag(&args[1..], "--socket").unwrap_or_else(default_socket_path),
        }),
        "module-api" => parse_module_api_args(&args[1..]),
        "runtime-config" => Ok(Command::RuntimeConfig {
            config: require_flag(&args[1..], "--config")?,
        }),
        "config-value" => Ok(Command::ConfigValue {
            config: require_flag(&args[1..], "--config")?,
            key: require_flag(&args[1..], "--key")?,
            fallback: optional_flag(&args[1..], "--fallback").unwrap_or_default(),
            list: false,
        }),
        "config-list" => Ok(Command::ConfigValue {
            config: require_flag(&args[1..], "--config")?,
            key: require_flag(&args[1..], "--key")?,
            fallback: optional_flag(&args[1..], "--fallback").unwrap_or_default(),
            list: true,
        }),
        "payload-fields" => Ok(Command::PayloadFields {
            payload: optional_flag(&args[1..], "--payload"),
        }),
        "model-output" => Ok(Command::ModelOutput {
            config: require_flag(&args[1..], "--config")?,
            prompt: optional_flag(&args[1..], "--prompt"),
            debug: args.iter().any(|arg| arg == "--debug"),
        }),
        "execution-run" => Ok(Command::ExecutionRun {
            config: require_flag(&args[1..], "--config")?,
            request_json: optional_flag(&args[1..], "--request"),
            debug: args.iter().any(|arg| arg == "--debug"),
            stream: args.iter().any(|arg| arg == "--stream"),
        }),
        "execution-resume-permission" => Ok(Command::ExecutionResumePermission {
            config: require_flag(&args[1..], "--config")?,
            permission_id: require_flag(&args[1..], "--permission-id")?,
            decision: require_flag(&args[1..], "--decision")?,
            debug: args.iter().any(|arg| arg == "--debug"),
            stream: args.iter().any(|arg| arg == "--stream"),
        }),
        "memory-context" => Ok(Command::MemoryContext {
            config: require_flag(&args[1..], "--config")?,
            plugin: require_flag(&args[1..], "--plugin")?,
        }),
        "memory-record-turns" => Ok(Command::MemoryRecordTurns {
            config: require_flag(&args[1..], "--config")?,
            plugin: require_flag(&args[1..], "--plugin")?,
            user_text: require_flag(&args[1..], "--user-text")?,
            payload_json: optional_flag(&args[1..], "--payload"),
            debug: args.iter().any(|arg| arg == "--debug"),
        }),
        "workspace-context" => Ok(Command::WorkspaceContext {
            cwd: require_flag(&args[1..], "--cwd")?,
        }),
        "tool-list" => Ok(Command::ToolList {
            config: require_flag(&args[1..], "--config")?,
            plugin: optional_flag(&args[1..], "--plugin").unwrap_or_else(|| "chat".into()),
        }),
        "tool-call" => Ok(Command::ToolCall {
            config: require_flag(&args[1..], "--config")?,
            tool: require_flag(&args[1..], "--tool")?,
            args_json: optional_flag(&args[1..], "--args").unwrap_or_else(|| "{}".into()),
        }),
        "tool-batch" => Ok(Command::ToolBatch {
            config: require_flag(&args[1..], "--config")?,
            calls_json: optional_flag(&args[1..], "--calls").unwrap_or_else(|| "[]".into()),
        }),
        "task-list" => Ok(Command::TaskList {
            config: require_flag(&args[1..], "--config")?,
            status: optional_flag(&args[1..], "--status").unwrap_or_default(),
            limit: optional_flag(&args[1..], "--limit")
                .unwrap_or_else(|| "20".into())
                .parse::<usize>()
                .map_err(|err| err.to_string())?,
        }),
        "task-show" => Ok(Command::TaskShow {
            config: require_flag(&args[1..], "--config")?,
            task_id: require_flag(&args[1..], "--task-id")?,
        }),
        "task-resume" => Ok(Command::TaskResume {
            config: require_flag(&args[1..], "--config")?,
            task_id: require_flag(&args[1..], "--task-id")?,
        }),
        "task-cancel" => Ok(Command::TaskCancel {
            config: require_flag(&args[1..], "--config")?,
            task_id: require_flag(&args[1..], "--task-id")?,
        }),
        _ => Ok(Command::Run {
            mode: require_flag(&args, "--mode")?,
            input: require_flag(&args, "--input")?,
            cwd: require_flag(&args, "--cwd")?,
            shell: optional_flag(&args, "--shell").unwrap_or_else(|| "zsh".into()),
            exit_status: optional_flag(&args, "--exit-status")
                .unwrap_or_else(|| "0".into())
                .parse::<i64>()
                .map_err(|err| err.to_string())?,
            recent_command: optional_flag(&args, "--recent-command").unwrap_or_default(),
            selected_command: optional_flag(&args, "--selected-command").unwrap_or_default(),
            config: optional_flag(&args, "--config")
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            stream: args.iter().any(|arg| arg == "--stream"),
        }),
    }
}

fn parse_module_api_args(args: &[String]) -> Result<Command, String> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err("module-api requires a subcommand".into());
    };
    match subcommand {
        "info" => Ok(Command::ModuleApiInfo),
        "model-output" => Ok(Command::ModelOutput {
            config: require_flag(&args[1..], "--config")?,
            prompt: optional_flag(&args[1..], "--prompt"),
            debug: args.iter().any(|arg| arg == "--debug"),
        }),
        "execution-run" => Ok(Command::ExecutionRun {
            config: require_flag(&args[1..], "--config")?,
            request_json: optional_flag(&args[1..], "--request"),
            debug: args.iter().any(|arg| arg == "--debug"),
            stream: args.iter().any(|arg| arg == "--stream"),
        }),
        "execution-resume-permission" => Ok(Command::ExecutionResumePermission {
            config: require_flag(&args[1..], "--config")?,
            permission_id: require_flag(&args[1..], "--permission-id")?,
            decision: require_flag(&args[1..], "--decision")?,
            debug: args.iter().any(|arg| arg == "--debug"),
            stream: args.iter().any(|arg| arg == "--stream"),
        }),
        "memory-context" => Ok(Command::MemoryContext {
            config: require_flag(&args[1..], "--config")?,
            plugin: require_flag(&args[1..], "--plugin")?,
        }),
        "memory-record-turns" => Ok(Command::MemoryRecordTurns {
            config: require_flag(&args[1..], "--config")?,
            plugin: require_flag(&args[1..], "--plugin")?,
            user_text: require_flag(&args[1..], "--user-text")?,
            payload_json: optional_flag(&args[1..], "--payload"),
            debug: args.iter().any(|arg| arg == "--debug"),
        }),
        "workspace-context" => Ok(Command::WorkspaceContext {
            cwd: require_flag(&args[1..], "--cwd")?,
        }),
        "tool-list" => Ok(Command::ToolList {
            config: require_flag(&args[1..], "--config")?,
            plugin: optional_flag(&args[1..], "--plugin").unwrap_or_else(|| "chat".into()),
        }),
        "tool-call" => Ok(Command::ToolCall {
            config: require_flag(&args[1..], "--config")?,
            tool: require_flag(&args[1..], "--tool")?,
            args_json: optional_flag(&args[1..], "--args").unwrap_or_else(|| "{}".into()),
        }),
        "tool-batch" => Ok(Command::ToolBatch {
            config: require_flag(&args[1..], "--config")?,
            calls_json: optional_flag(&args[1..], "--calls").unwrap_or_else(|| "[]".into()),
        }),
        other => Err(format!("unknown module-api subcommand: {other}")),
    }
}

fn optional_flag(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find_map(|window| {
        if window[0] == flag {
            Some(window[1].clone())
        } else {
            None
        }
    })
}

fn require_flag(args: &[String], flag: &str) -> Result<String, String> {
    optional_flag(args, flag).ok_or_else(|| format!("missing {flag}"))
}

fn default_socket_path() -> String {
    env::var("NOODLE_SOCKET").unwrap_or_else(|_| "~/.noodle/noodle.sock".into())
}

fn default_pid_path() -> String {
    env::var("NOODLE_PIDFILE").unwrap_or_else(|_| "~/.noodle/noodle.pid".into())
}

fn default_memory_path() -> String {
    env::var("NOODLE_MEMORY_DB").unwrap_or_else(|_| "~/.noodle/memory.db".into())
}

fn module_api_capabilities() -> Vec<String> {
    vec![
        "info".into(),
        "stream_envelopes_v1".into(),
        "model_output".into(),
        "execution_run".into(),
        "execution_resume_permission".into(),
        "memory_context".into(),
        "memory_record_turns".into(),
        "workspace_context".into(),
        "tool_list".into(),
        "tool_call".into(),
        "tool_batch".into(),
    ]
}

fn module_api_info_json(binary_path: &str) -> Value {
    json!({
        "version": MODULE_API_VERSION,
        "command_prefix": [binary_path, "module-api"],
        "capabilities": module_api_capabilities(),
    })
}

fn execute_via_daemon(command: Command) -> Result<String, String> {
    if env::var("NOODLE_BYPASS_DAEMON").ok().as_deref() == Some("1") {
        return execute_local(command);
    }
    if matches!(
        command,
        Command::PayloadFields { .. }
            | Command::ModuleApiInfo
            | Command::ModelOutput { .. }
            | Command::ExecutionRun { .. }
            | Command::ExecutionResumePermission { .. }
            | Command::MemoryContext { .. }
            | Command::MemoryRecordTurns { .. }
            | Command::WorkspaceContext { .. }
    ) {
        return execute_local(command);
    }

    let socket = default_socket_path();
    let request = to_request(command);
    let response =
        send_request(&socket, &request).map_err(|err| daemon_unavailable_error(&socket, &err))?;
    if response.ok {
        Ok(response.output)
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "daemon request failed".into()))
    }
}

fn execute_streaming(command: Command) -> Result<(), String> {
    if env::var("NOODLE_BYPASS_DAEMON").ok().as_deref() == Some("1")
        || matches!(
            command,
            Command::ExecutionRun { .. } | Command::ExecutionResumePermission { .. }
        )
    {
        return execute_local_stream(command, &mut |payload| {
            println!("{payload}");
            io::stdout().flush().map_err(|err| err.to_string())
        });
    }

    let socket = default_socket_path();
    let request = to_request(command);
    let mut stream = open_stream_request(&socket, &request)
        .map_err(|err| daemon_unavailable_error(&socket, &err))?;
    let mut reader = io::BufReader::new(&mut stream);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).map_err(|err| err.to_string())?;
        if bytes == 0 {
            break;
        }
        print!("{line}");
        io::stdout().flush().map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn read_optional_text(value: Option<String>) -> Result<String, String> {
    match value {
        Some(value) => Ok(value),
        None => {
            let mut stdin = String::new();
            io::stdin()
                .read_to_string(&mut stdin)
                .map_err(|err| err.to_string())?;
            Ok(stdin)
        }
    }
}

fn debug_enabled(config: &Value, explicit_debug: bool) -> bool {
    explicit_debug
        || value_or_env(config, "NOODLE_DEBUG", "runtime.debug", "0") != "0"
        || value_or_env(config, "NOODLE_DEBUG", "debug", "0") != "0"
}

fn execution_request_from_json(
    config: &Value,
    request_json: &str,
) -> Result<engine::ChatExecutionConfig, String> {
    let request: ExternalExecutionRequestInput =
        serde_json::from_str(request_json).map_err(|err| err.to_string())?;
    let plugin = request.plugin.trim();
    if plugin.is_empty() {
        return Err("execution request requires plugin".into());
    }
    Ok(engine::ChatExecutionConfig {
        plugin: plugin.to_string(),
        input: request.input,
        working_directory: request.working_directory,
        base_prompt: request.base_prompt,
        memory_context: request.memory_context,
        include_tool_context: request
            .include_tool_context
            .unwrap_or_else(|| plugin_include_tool_context(config, plugin)),
        tool_calling_enabled: request
            .tool_calling_enabled
            .unwrap_or_else(|| engine::plugin_tool_calling_enabled(config, plugin)),
        task_execution_enabled: request
            .task_execution_enabled
            .unwrap_or_else(|| engine::plugin_task_execution_enabled(config, plugin)),
        max_tool_rounds: request
            .max_tool_rounds
            .unwrap_or_else(|| engine::plugin_max_tool_rounds(config, plugin)),
        max_replans: request
            .max_replans
            .unwrap_or_else(|| engine::plugin_max_replans(config, plugin)),
        available_tools: engine::plugin_tools_for_config(config, plugin),
        granted_tool_names: Vec::new(),
    })
}

fn emit_stream_envelope(
    emit_line: &mut dyn FnMut(String) -> Result<(), String>,
    kind: &str,
    payload: Option<Value>,
    error: Option<String>,
) -> Result<(), String> {
    emit_line(
        serde_json::to_string(&json!({
            "type": kind,
            "ok": error.is_none(),
            "payload": payload,
            "error": error,
        }))
        .map_err(|err| err.to_string())?,
    )
}

fn execute_external_execution(
    config: &Value,
    request_json: &str,
    debug: bool,
    streaming: bool,
    emitter: &mut dyn FnMut(&actions::DaemonAction) -> Result<(), String>,
) -> Result<Value, String> {
    let request = execution_request_from_json(config, request_json)?;
    let action = engine::run_chat_execution(config, request, streaming, emitter, &|prompt| {
        model_output(config, prompt, debug)
    })?;
    Ok(action.into_value())
}

fn execute_permission_resume(
    config: &Value,
    permission_id: &str,
    decision: &str,
    debug: bool,
    streaming: bool,
    emitter: &mut dyn FnMut(&actions::DaemonAction) -> Result<(), String>,
) -> Result<Value, String> {
    let action = engine::resume_chat_execution_from_permission(
        config,
        permission_id,
        decision,
        streaming,
        emitter,
        &|prompt| model_output(config, prompt, debug),
    )?;
    Ok(action.into_value())
}

fn daemon_unavailable_error(socket: &str, cause: &str) -> String {
    let socket_path = expand_home(socket);
    format!(
        "noodle daemon is not running or unreachable at {} ({cause}). Manage it with launchctl, for example: launchctl kickstart -k gui/$(id -u)/com.noodle.daemon",
        socket_path.display()
    )
}

fn serve_daemon(socket: &str) -> Result<(), String> {
    let socket_path = expand_home(socket);
    let pid_path = expand_home(&default_pid_path());
    let parent = socket_path
        .parent()
        .ok_or_else(|| "invalid socket path".to_string())?;
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    if let Some(pid_parent) = pid_path.parent() {
        fs::create_dir_all(pid_parent).map_err(|err| err.to_string())?;
    }
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }
    fs::write(&pid_path, std::process::id().to_string()).map_err(|err| err.to_string())?;
    let listener = UnixListener::bind(&socket_path).map_err(|err| err.to_string())?;
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let _ = handle_stream(&mut stream);
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

fn handle_stream(stream: &mut UnixStream) -> Result<(), String> {
    let mut body = String::new();
    let result = stream.read_to_string(&mut body);
    if let Err(err) = result {
        let response = serialize_response(false, "", Some(err.to_string()));
        stream
            .write_all(response.as_bytes())
            .map_err(|write_err| write_err.to_string())?;
        return Ok(());
    }

    let request = match serde_json::from_str::<DaemonRequest>(&body) {
        Ok(request) => request,
        Err(err) => {
            let response = serialize_response(false, "", Some(err.to_string()));
            stream
                .write_all(response.as_bytes())
                .map_err(|write_err| write_err.to_string())?;
            return Ok(());
        }
    };

    if request.stream.unwrap_or(false) {
        return handle_streaming_request(stream, request);
    }

    let response = match execute_local(from_request(request)) {
        Ok(output) => serialize_response(true, &output, None),
        Err(err) => serialize_response(false, "", Some(err)),
    };
    stream
        .write_all(response.as_bytes())
        .map_err(|err| err.to_string())
}

fn handle_streaming_request(stream: &mut UnixStream, request: DaemonRequest) -> Result<(), String> {
    let command = from_request(request);
    let mut writer = io::BufWriter::new(stream);
    let result = execute_local_stream(command, &mut |line| {
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| writer.flush())
            .map_err(|err| err.to_string())
    });
    if let Err(err) = result {
        let payload = json!({
            "plugin": "host",
            "action": "ask",
            "question": err,
        });
        writer
            .write_all(
                serde_json::to_string(&payload)
                    .map_err(|json_err| json_err.to_string())?
                    .as_bytes(),
            )
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| writer.flush())
            .map_err(|write_err| write_err.to_string())?;
    }
    Ok(())
}

fn serve_mcp_stdio() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|err| err.to_string())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: Value = serde_json::from_str(trimmed).map_err(|err| err.to_string())?;
        if let Some(items) = request.as_array() {
            let mut responses = Vec::new();
            for item in items {
                if let Some(response) = handle_mcp_message(item)? {
                    responses.push(response);
                }
            }
            if !responses.is_empty() {
                writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&responses).map_err(|err| err.to_string())?
                )
                .map_err(|err| err.to_string())?;
                stdout.flush().map_err(|err| err.to_string())?;
            }
            continue;
        }

        if let Some(response) = handle_mcp_message(&request)? {
            writeln!(
                stdout,
                "{}",
                serde_json::to_string(&response).map_err(|err| err.to_string())?
            )
            .map_err(|err| err.to_string())?;
            stdout.flush().map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn handle_mcp_message(request: &Value) -> Result<Option<Value>, String> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": "noodle",
                    "version": "0.1.0"
                }
            }
        }))),
        "notifications/initialized" => Ok(None),
        "ping" => Ok(id.map(|request_id| {
            json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {}
            })
        })),
        "tools/list" => Ok(id.map(|request_id| {
            let config_path =
                env::var("NOODLE_CONFIG").unwrap_or_else(|_| "~/.noodle/config.json".into());
            let config = load_config(&config_path);
            let tools = exported_mcp_tools(&config)
                .into_iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "tools": tools }
            })
        })),
        "tools/call" => {
            let response = match handle_mcp_tool_call(&params) {
                Ok(result) => id.map(|request_id| {
                    json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": result
                    })
                }),
                Err(err) => id.map(|request_id| {
                    json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {
                            "code": -32000,
                            "message": err
                        }
                    })
                }),
            };
            Ok(response)
        }
        _ => Ok(id.map(|request_id| {
            json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32601,
                    "message": format!("method not found: {method}")
                }
            })
        })),
    }
}

fn handle_mcp_tool_call(params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing tool name".to_string())?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let config_path = env::var("NOODLE_CONFIG").unwrap_or_else(|_| "~/.noodle/config.json".into());
    let config = load_config(&config_path);
    if !exported_mcp_tools(&config)
        .iter()
        .any(|tool| tool.name == name)
    {
        return Err(format!("unknown tool: {name}"));
    }

    match name.split('.').next().unwrap_or("") {
        "chat" if name == "chat.send" => {
            let message = arguments
                .get("message")
                .and_then(Value::as_str)
                .ok_or_else(|| "chat.send requires message".to_string())?;
            let debug = debug_enabled(&config, false);
            let cwd = env::current_dir()
                .ok()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            let payload = dispatch_event(
                &config,
                "command_not_found",
                &format!("oo {message}"),
                &cwd,
                "mcp",
                0,
                "",
                "",
                debug,
            )?;
            let reply = payload_text(&payload);
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": reply
                    }
                ],
                "isError": false
            }))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn payload_text(payload: &Value) -> String {
    match payload.get("action").and_then(Value::as_str) {
        Some("message") => payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some("ask") => payload
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some("batch") => payload
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.last())
            .map(payload_text)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn serialize_response(ok: bool, output: &str, error: Option<String>) -> String {
    serde_json::to_string(&DaemonResponse {
        ok,
        output: output.into(),
        error,
    })
    .unwrap_or_else(|_| "{\"ok\":false,\"output\":\"\",\"error\":\"serialization failed\"}".into())
}

fn send_request(socket: &str, request: &DaemonRequest) -> Result<DaemonResponse, String> {
    let mut stream = open_stream_request(socket, request)?;
    let mut response_body = String::new();
    stream
        .read_to_string(&mut response_body)
        .map_err(|err| err.to_string())?;
    serde_json::from_str::<DaemonResponse>(&response_body).map_err(|err| err.to_string())
}

fn open_stream_request(socket: &str, request: &DaemonRequest) -> Result<UnixStream, String> {
    let socket_path = expand_home(socket);
    let mut stream = UnixStream::connect(&socket_path).map_err(|err| err.to_string())?;
    let body = serde_json::to_vec(request).map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|err| err.to_string())?;
    Ok(stream)
}

fn to_request(command: Command) -> DaemonRequest {
    match command {
        Command::Run {
            mode,
            input,
            cwd,
            shell,
            exit_status,
            recent_command,
            selected_command,
            config,
            stream,
        } => DaemonRequest {
            command: "run".into(),
            mode: Some(mode),
            input: Some(input),
            cwd: Some(cwd),
            shell: Some(shell),
            exit_status: Some(exit_status),
            recent_command: Some(recent_command),
            selected_command: Some(selected_command),
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: None,
            status: None,
            limit: None,
            stream: Some(stream),
        },
        Command::RuntimeConfig { config } => DaemonRequest {
            command: "runtime-config".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: None,
            status: None,
            limit: None,
            stream: None,
        },
        Command::ConfigValue {
            config,
            key,
            fallback,
            list,
        } => DaemonRequest {
            command: if list { "config-list" } else { "config-value" }.into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: Some(key),
            fallback: Some(fallback),
            list: Some(list),
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: None,
            status: None,
            limit: None,
            stream: None,
        },
        Command::PayloadFields { payload } => DaemonRequest {
            command: "payload-fields".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: None,
            key: None,
            fallback: None,
            list: None,
            payload,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: None,
            status: None,
            limit: None,
            stream: None,
        },
        Command::ModuleApiInfo
        | Command::ModelOutput { .. }
        | Command::ExecutionRun { .. }
        | Command::ExecutionResumePermission { .. }
        | Command::MemoryContext { .. }
        | Command::MemoryRecordTurns { .. }
        | Command::WorkspaceContext { .. } => unreachable!(),
        Command::ToolList { config, plugin } => DaemonRequest {
            command: "tool-list".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: Some(plugin),
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: None,
            status: None,
            limit: None,
            stream: None,
        },
        Command::ToolCall {
            config,
            tool,
            args_json,
        } => DaemonRequest {
            command: "tool-call".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: Some(tool),
            args_json: Some(args_json),
            calls_json: None,
            task_id: None,
            status: None,
            limit: None,
            stream: None,
        },
        Command::ToolBatch { config, calls_json } => DaemonRequest {
            command: "tool-batch".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: Some(calls_json),
            task_id: None,
            status: None,
            limit: None,
            stream: None,
        },
        Command::TaskList {
            config,
            status,
            limit,
        } => DaemonRequest {
            command: "task-list".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: None,
            status: Some(status),
            limit: Some(limit),
            stream: None,
        },
        Command::TaskShow { config, task_id } => DaemonRequest {
            command: "task-show".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: Some(task_id),
            status: None,
            limit: None,
            stream: None,
        },
        Command::TaskResume { config, task_id } => DaemonRequest {
            command: "task-resume".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: Some(task_id),
            status: None,
            limit: None,
            stream: None,
        },
        Command::TaskCancel { config, task_id } => DaemonRequest {
            command: "task-cancel".into(),
            mode: None,
            input: None,
            cwd: None,
            shell: None,
            exit_status: None,
            recent_command: None,
            selected_command: None,
            config: Some(config),
            key: None,
            fallback: None,
            list: None,
            payload: None,
            plugin: None,
            tool: None,
            args_json: None,
            calls_json: None,
            task_id: Some(task_id),
            status: None,
            limit: None,
            stream: None,
        },
        Command::Mcp => unreachable!(),
        Command::Daemon { .. } => unreachable!(),
    }
}

fn from_request(request: DaemonRequest) -> Command {
    match request.command.as_str() {
        "runtime-config" => Command::RuntimeConfig {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
        },
        "config-list" => Command::ConfigValue {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            key: request.key.unwrap_or_default(),
            fallback: request.fallback.unwrap_or_default(),
            list: true,
        },
        "config-value" => Command::ConfigValue {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            key: request.key.unwrap_or_default(),
            fallback: request.fallback.unwrap_or_default(),
            list: false,
        },
        "payload-fields" => Command::PayloadFields {
            payload: request.payload,
        },
        "module-api-info"
        | "model-output"
        | "execution-run"
        | "execution-resume-permission"
        | "memory-context"
        | "memory-record-turns"
        | "workspace-context" => unreachable!(),
        "tool-list" => Command::ToolList {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            plugin: request.plugin.unwrap_or_else(|| "chat".into()),
        },
        "tool-call" => Command::ToolCall {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            tool: request.tool.unwrap_or_default(),
            args_json: request.args_json.unwrap_or_else(|| "{}".into()),
        },
        "tool-batch" => Command::ToolBatch {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            calls_json: request.calls_json.unwrap_or_else(|| "[]".into()),
        },
        "task-list" => Command::TaskList {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            status: request.status.unwrap_or_default(),
            limit: request.limit.unwrap_or(20),
        },
        "task-show" => Command::TaskShow {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            task_id: request.task_id.unwrap_or_default(),
        },
        "task-resume" => Command::TaskResume {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            task_id: request.task_id.unwrap_or_default(),
        },
        "task-cancel" => Command::TaskCancel {
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            task_id: request.task_id.unwrap_or_default(),
        },
        _ => Command::Run {
            mode: request.mode.unwrap_or_else(|| "command_not_found".into()),
            input: request.input.unwrap_or_default(),
            cwd: request.cwd.unwrap_or_default(),
            shell: request.shell.unwrap_or_else(|| "zsh".into()),
            exit_status: request.exit_status.unwrap_or(0),
            recent_command: request.recent_command.unwrap_or_default(),
            selected_command: request.selected_command.unwrap_or_default(),
            config: request
                .config
                .unwrap_or_else(|| "~/.noodle/config.json".into()),
            stream: request.stream.unwrap_or(false),
        },
    }
}

fn execute_local(command: Command) -> Result<String, String> {
    let result = match command {
        Command::RuntimeConfig { config } => {
            let config = load_config(&config);
            Ok(render_runtime_config(&config))
        }
        Command::ConfigValue {
            config,
            key,
            fallback,
            list,
        } => {
            let config = load_config(&config);
            if list {
                if let Some(items) = lookup(&config, &key).and_then(Value::as_array) {
                    Ok(format!(
                        "{}\n",
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ))
                } else {
                    Ok(format!("{}\n", value_or_env(&config, "", &key, &fallback)))
                }
            } else {
                Ok(format!("{}\n", value_or_env(&config, "", &key, &fallback)))
            }
        }
        Command::PayloadFields { payload } => {
            let payload = read_optional_text(payload)?;
            render_payload_fields(&payload)
        }
        Command::ModuleApiInfo => {
            let binary_path = env::current_exe()
                .ok()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "noodle".into());
            serde_json::to_string(&module_api_info_json(&binary_path))
                .map_err(|err| err.to_string())
        }
        Command::ModelOutput {
            config,
            prompt,
            debug,
        } => {
            let prompt = read_optional_text(prompt)?;
            let config = load_config(&config);
            Ok(model_output(
                &config,
                &prompt,
                debug_enabled(&config, debug),
            )?)
        }
        Command::ExecutionRun {
            config,
            request_json,
            debug,
            ..
        } => {
            let request_json = read_optional_text(request_json)?;
            let config = load_config(&config);
            let debug = debug_enabled(&config, debug);
            let mut noop = |_action: &actions::DaemonAction| Ok(());
            let payload =
                execute_external_execution(&config, &request_json, debug, false, &mut noop)?;
            serde_json::to_string(&payload).map_err(|err| err.to_string())
        }
        Command::ExecutionResumePermission {
            config,
            permission_id,
            decision,
            debug,
            ..
        } => {
            let config = load_config(&config);
            let debug = debug_enabled(&config, debug);
            let mut noop = |_action: &actions::DaemonAction| Ok(());
            let payload = execute_permission_resume(
                &config,
                &permission_id,
                &decision,
                debug,
                false,
                &mut noop,
            )?;
            serde_json::to_string(&payload).map_err(|err| err.to_string())
        }
        Command::MemoryContext { config, plugin } => {
            let config = load_config(&config);
            Ok(memory_plugin_prompt_context(&config, &plugin)?)
        }
        Command::MemoryRecordTurns {
            config,
            plugin,
            user_text,
            payload_json,
            debug,
        } => {
            let payload_json = read_optional_text(payload_json)?;
            let config = load_config(&config);
            let payload =
                serde_json::from_str::<Value>(&payload_json).map_err(|err| err.to_string())?;
            record_turn_memory(
                &config,
                &plugin,
                &user_text,
                &payload,
                debug_enabled(&config, debug),
            )?;
            Ok(String::new())
        }
        Command::WorkspaceContext { cwd } => {
            let sections = workspace_prompt_sections(&cwd);
            serde_json::to_string(&sections).map_err(|err| err.to_string())
        }
        Command::ToolList { config, plugin } => {
            let config = load_config(&config);
            let tools = tools_for_plugin(&config, &plugin)
                .into_iter()
                .map(tool_json)
                .collect::<Vec<_>>();
            serde_json::to_string(&json!({"plugin": plugin, "tools": tools}))
                .map_err(|err| err.to_string())
        }
        Command::ToolCall {
            config,
            tool,
            args_json,
        } => {
            let config = load_config(&config);
            let args = serde_json::from_str::<Value>(&args_json).map_err(|err| err.to_string())?;
            let result = invoke_builtin_tool(&config, None, &tool, &args)?;
            serde_json::to_string(&result).map_err(|err| err.to_string())
        }
        Command::ToolBatch { config, calls_json } => {
            let config = load_config(&config);
            let calls =
                serde_json::from_str::<Value>(&calls_json).map_err(|err| err.to_string())?;
            let items = calls
                .as_array()
                .ok_or_else(|| "tool-batch requires a JSON array".to_string())?;
            let mut results = Vec::new();
            for item in items {
                let tool = item
                    .get("tool")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "tool-batch call requires tool".to_string())?;
                let args = item.get("args").cloned().unwrap_or_else(|| json!({}));
                let resolved_args = resolve_batch_args(&args, &results);
                let result = invoke_builtin_tool(&config, None, tool, &resolved_args)?;
                results.push(serde_json::to_value(result).map_err(|err| err.to_string())?);
            }
            serde_json::to_string(&json!({"results": results})).map_err(|err| err.to_string())
        }
        Command::TaskList {
            config,
            status,
            limit,
        } => {
            let config = load_config(&config);
            let tasks = list_task_records(
                &config,
                limit.max(1),
                if status.trim().is_empty() {
                    None
                } else {
                    Some(status.trim())
                },
            )?;
            serde_json::to_string(&json!({"tasks": tasks})).map_err(|err| err.to_string())
        }
        Command::TaskShow { config, task_id } => {
            let config = load_config(&config);
            let task = load_task_record(&config, &task_id)?;
            let runtime = load_task_runtime_state(&config, &task_id)?;
            serde_json::to_string(&json!({"task": task, "runtime": runtime}))
                .map_err(|err| err.to_string())
        }
        Command::TaskResume { config, task_id } => {
            let config = load_config(&config);
            let debug = debug_enabled(&config, false);
            let mut noop = |_action: &actions::DaemonAction| Ok(());
            let payload =
                engine::resume_task_execution(&config, &task_id, false, &mut noop, &|prompt| {
                    model_output(&config, prompt, debug)
                })?;
            serde_json::to_string(&payload.into_value()).map_err(|err| err.to_string())
        }
        Command::TaskCancel { config, task_id } => {
            let config = load_config(&config);
            let task = cancel_task(&config, &task_id)?;
            serde_json::to_string(&json!({"task": task})).map_err(|err| err.to_string())
        }
        Command::Run {
            mode,
            input,
            cwd,
            shell,
            exit_status,
            recent_command,
            selected_command,
            config,
            ..
        } => {
            let config = load_config(&config);
            let debug = debug_enabled(&config, false);
            let payload = dispatch_event(
                &config,
                &mode,
                &input,
                &cwd,
                &shell,
                exit_status,
                &recent_command,
                &selected_command,
                debug,
            )?;
            serde_json::to_string(&payload).map_err(|err| err.to_string())
        }
        Command::Mcp => unreachable!(),
        Command::Daemon { .. } => unreachable!(),
    };
    finalize_local_command(result)
}

fn execute_local_stream(
    command: Command,
    emit_line: &mut dyn FnMut(String) -> Result<(), String>,
) -> Result<(), String> {
    let result = match command {
        Command::Run {
            mode,
            input,
            cwd,
            shell,
            exit_status,
            recent_command,
            selected_command,
            config,
            ..
        } => {
            let config = load_config(&config);
            let debug = value_or_env(&config, "NOODLE_DEBUG", "runtime.debug", "0") != "0"
                || value_or_env(&config, "NOODLE_DEBUG", "debug", "0") != "0";
            let mut emit_action = |action: &actions::DaemonAction| {
                emit_line(
                    serde_json::to_string(&action.clone().into_value())
                        .map_err(|err| err.to_string())?,
                )
            };
            let final_payload = dispatch_event_streaming(
                &config,
                &mode,
                &input,
                &cwd,
                &shell,
                exit_status,
                &recent_command,
                &selected_command,
                debug,
                &mut emit_action,
            )?;
            emit_line(serde_json::to_string(&final_payload).map_err(|err| err.to_string())?)
        }
        Command::ExecutionRun {
            config,
            request_json,
            debug,
            ..
        } => {
            let request_json = read_optional_text(request_json)?;
            let config = load_config(&config);
            let debug = debug_enabled(&config, debug);
            let mut emit_action = |action: &actions::DaemonAction| {
                emit_stream_envelope(emit_line, "action", Some(action.clone().into_value()), None)
            };
            let final_payload =
                execute_external_execution(&config, &request_json, debug, true, &mut emit_action)?;
            emit_stream_envelope(emit_line, "final", Some(final_payload), None)
        }
        Command::ExecutionResumePermission {
            config,
            permission_id,
            decision,
            debug,
            ..
        } => {
            let config = load_config(&config);
            let debug = debug_enabled(&config, debug);
            let mut emit_action = |action: &actions::DaemonAction| {
                emit_stream_envelope(emit_line, "action", Some(action.clone().into_value()), None)
            };
            let final_payload = execute_permission_resume(
                &config,
                &permission_id,
                &decision,
                debug,
                true,
                &mut emit_action,
            )?;
            emit_stream_envelope(emit_line, "final", Some(final_payload), None)
        }
        other => {
            let output = execute_local(other)?;
            emit_line(output)
        }
    };
    finalize_local_command(result)
}

fn finalize_local_command<T>(result: Result<T, String>) -> Result<T, String> {
    let cleanup = shutdown_all_mcp_sessions();
    match result {
        Ok(value) => cleanup.map(|_| value),
        Err(err) => Err(err),
    }
}

fn resolve_batch_args(value: &Value, previous_results: &[Value]) -> Value {
    match value {
        Value::String(text) => {
            if let Some(resolved) = resolve_batch_reference(text, previous_results) {
                resolved
            } else {
                value.clone()
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_batch_args(item, previous_results))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), resolve_batch_args(value, previous_results)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn resolve_batch_reference(text: &str, previous_results: &[Value]) -> Option<Value> {
    let rest = text.strip_prefix("__FROM_RESULT_")?;
    let (index, path) = rest.split_once("__.")?;
    let index = index.parse::<usize>().ok()?;
    let mut current = previous_results.get(index)?;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current.clone())
}

fn load_config(path: &str) -> Value {
    let expanded = expand_home(path);
    let modified = fs::metadata(&expanded)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    if let Ok(cache) = config_cache().lock() {
        if let Some(cached) = cache.get(&expanded) {
            if cached.modified == modified {
                return config_with_meta(cached.value.clone(), &expanded);
            }
        }
    }

    let value = fs::read_to_string(&expanded)
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .unwrap_or_else(|| json!({}));
    if let Ok(mut cache) = config_cache().lock() {
        cache.insert(
            expanded.clone(),
            CachedConfig {
                modified,
                value: value.clone(),
            },
        );
    }
    config_with_meta(value, &expanded)
}

fn config_cache() -> &'static Mutex<HashMap<PathBuf, CachedConfig>> {
    static CONFIG_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedConfig>>> = OnceLock::new();
    CONFIG_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn config_with_meta(mut value: Value, path: &Path) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "_meta".into(),
            json!({
                "config_path": path.to_string_lossy().to_string()
            }),
        );
    }
    value
}

pub(crate) fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}

pub(crate) fn lookup<'a>(config: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = config;
    for part in key.split('.') {
        if part.is_empty() {
            continue;
        }
        current = current.get(part)?;
    }
    Some(current)
}

fn value_or_env(config: &Value, env_name: &str, key: &str, default: &str) -> String {
    if !env_name.is_empty() {
        if let Ok(value) = env::var(env_name) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    match lookup(config, key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => {
            if *value {
                "1".into()
            } else {
                "0".into()
            }
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<Vec<_>>()
            .join(" "),
        Some(Value::Null) | None => default.into(),
        Some(other) => other.to_string(),
    }
}

fn strip_terminal_control_sequences(input: &str) -> String {
    let mut cleaned = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    let mut previous_was_escape = false;
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' || (previous_was_escape && next == '\\') {
                            break;
                        }
                        previous_was_escape = next == '\u{1b}';
                    }
                }
                Some(_) => {}
                None => {}
            }
            continue;
        }

        if ch.is_control() || ch == '\u{7f}' {
            continue;
        }

        cleaned.push(ch);
    }

    cleaned.trim().to_string()
}

fn sanitized_header_value(value: &str) -> String {
    strip_terminal_control_sequences(value)
}

fn value_to_i64(config: &Value, key: &str, default: i64) -> i64 {
    match lookup(config, key) {
        Some(Value::Number(value)) => value.as_i64().unwrap_or(default),
        Some(Value::String(value)) => value.parse::<i64>().unwrap_or(default),
        Some(Value::Bool(value)) => {
            if *value {
                1
            } else {
                0
            }
        }
        _ => default,
    }
}

fn memory_path(config: &Value) -> String {
    value_or_env(
        config,
        "NOODLE_MEMORY_DB",
        "memory.path",
        &default_memory_path(),
    )
}

fn memory_recent_turn_limit(config: &Value, plugin: &str, default: i64) -> i64 {
    value_to_i64(
        config,
        &format!("memory.{plugin}.recent_turn_limit"),
        default,
    )
    .max(4)
}

fn memory_context_turn_limit(config: &Value, plugin: &str, default: i64) -> usize {
    value_to_i64(
        config,
        &format!("memory.{plugin}.context_turn_limit"),
        default,
    )
    .max(1) as usize
}

fn memory_summary_max_chars(config: &Value, plugin: &str, default: i64) -> usize {
    value_to_i64(
        config,
        &format!("memory.{plugin}.summary_max_chars"),
        default,
    )
    .max(200) as usize
}

fn memory_compile_after_events(config: &Value, plugin: &str, default: i64) -> i64 {
    value_to_i64(
        config,
        &format!("memory.{plugin}.compile_after_events"),
        default,
    )
    .max(2)
}

fn memory_compile_prompt(config: &Value, plugin: &str, default: &str) -> String {
    value_or_env(
        config,
        "",
        &format!("memory.{plugin}.compile_prompt"),
        default,
    )
}

fn plugin_memory_behavior(config: &Value, plugin: &str) -> PluginMemoryBehavior {
    match plugin {
        "chat" => PluginMemoryBehavior {
            event_limits: vec![("turn", memory_recent_turn_limit(config, "chat", 24))],
            compile: Some(CompilePolicy {
                event_kind: "turn",
                threshold: memory_compile_after_events(config, "chat", 8),
                artifact_kind: "session_summary",
            }),
        },
        _ => PluginMemoryBehavior {
            event_limits: vec![],
            compile: None,
        },
    }
}

pub(crate) fn memory_connection(config: &Value) -> Result<Connection, String> {
    let path = expand_home(&memory_path(config));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let conn = Connection::open(path).map_err(|err| err.to_string())?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS events (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          plugin TEXT NOT NULL,
          kind TEXT NOT NULL,
          key TEXT NOT NULL DEFAULT '',
          value_json TEXT NOT NULL,
          created_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_events_plugin_kind_created
          ON events(plugin, kind, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_events_plugin_key_created
          ON events(plugin, key, created_at DESC);

        CREATE TABLE IF NOT EXISTS state (
          plugin TEXT NOT NULL,
          key TEXT NOT NULL,
          value_json TEXT NOT NULL,
          updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
          PRIMARY KEY(plugin, key)
        );

        CREATE TABLE IF NOT EXISTS artifacts (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          plugin TEXT NOT NULL,
          kind TEXT NOT NULL,
          content TEXT NOT NULL,
          source_json TEXT NOT NULL DEFAULT '{}',
          active INTEGER NOT NULL DEFAULT 1,
          created_at INTEGER NOT NULL DEFAULT (unixepoch()),
          updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_artifacts_plugin_kind_active
          ON artifacts(plugin, kind, active, updated_at DESC, id DESC);
        ",
    )
    .map_err(|err| err.to_string())?;
    Ok(conn)
}

pub(crate) fn memory_append_event(
    config: &Value,
    plugin: &str,
    kind: &str,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    let conn = memory_connection(config)?;
    conn.execute(
        "INSERT INTO events(plugin, kind, key, value_json) VALUES (?1, ?2, ?3, ?4)",
        params![plugin, kind, key, value.to_string()],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

fn memory_trim_events(config: &Value, plugin: &str, kind: &str, keep: i64) -> Result<(), String> {
    let conn = memory_connection(config)?;
    conn.execute(
        "DELETE FROM events
         WHERE plugin = ?1 AND kind = ?2
           AND id NOT IN (
             SELECT id FROM events
             WHERE plugin = ?1 AND kind = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3
           )",
        params![plugin, kind, keep],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

pub(crate) fn memory_set_state(
    config: &Value,
    plugin: &str,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    let conn = memory_connection(config)?;
    conn.execute(
        "INSERT INTO state(plugin, key, value_json, updated_at)
         VALUES (?1, ?2, ?3, unixepoch())
         ON CONFLICT(plugin, key)
         DO UPDATE SET value_json = excluded.value_json, updated_at = unixepoch()",
        params![plugin, key, value.to_string()],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

pub(crate) fn memory_get_state(
    config: &Value,
    plugin: &str,
    key: &str,
) -> Result<Option<Value>, String> {
    let conn = memory_connection(config)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM state WHERE plugin = ?1 AND key = ?2",
            params![plugin, key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| err.to_string())?;
    match raw {
        Some(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|err| err.to_string()),
        None => Ok(None),
    }
}

pub(crate) fn memory_increment_state_counter(
    config: &Value,
    plugin: &str,
    key: &str,
) -> Result<i64, String> {
    let current = memory_get_state(config, plugin, key)?
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let next = current + 1;
    memory_set_state(config, plugin, key, &json!(next))?;
    Ok(next)
}

fn memory_set_state_counter(
    config: &Value,
    plugin: &str,
    key: &str,
    value: i64,
) -> Result<(), String> {
    memory_set_state(config, plugin, key, &json!(value))
}

fn memory_active_artifact_content(
    config: &Value,
    plugin: &str,
    kind: &str,
) -> Result<Option<String>, String> {
    let conn = memory_connection(config)?;
    conn.query_row(
        "SELECT content
         FROM artifacts
         WHERE plugin = ?1 AND kind = ?2 AND active = 1
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![plugin, kind],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|err| err.to_string())
}

pub(crate) fn memory_upsert_artifact(
    config: &Value,
    plugin: &str,
    kind: &str,
    content: &str,
    source: &Value,
) -> Result<(), String> {
    let conn = memory_connection(config)?;
    conn.execute(
        "UPDATE artifacts SET active = 0 WHERE plugin = ?1 AND kind = ?2 AND active = 1",
        params![plugin, kind],
    )
    .map_err(|err| err.to_string())?;
    conn.execute(
        "INSERT INTO artifacts(plugin, kind, content, source_json, active, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, unixepoch(), unixepoch())",
        params![plugin, kind, content, source.to_string()],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

fn memory_recent_event_values(
    config: &Value,
    plugin: &str,
    kind: &str,
    limit: i64,
) -> Result<Vec<(i64, String)>, String> {
    let conn = memory_connection(config)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, value_json
             FROM events
             WHERE plugin = ?1 AND kind = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![plugin, kind, limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| err.to_string())?;
    let mut values = Vec::new();
    for row in rows {
        values.push(row.map_err(|err| err.to_string())?);
    }
    values.reverse();
    Ok(values)
}

fn conversation_lines_from_event_values(items: &[(i64, String)]) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for (_, raw) in items {
        let value: Value = serde_json::from_str(raw).map_err(|err| err.to_string())?;
        let role = value
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let text = value.get("text").and_then(Value::as_str).unwrap_or("");
        if !text.is_empty() {
            lines.push(format!("{role}: {}", text.replace('\n', " ")));
        }
    }
    Ok(lines)
}

fn compile_plugin_memory(config: &Value, plugin: &str, debug: bool) -> Result<(), String> {
    match plugin {
        "chat" => compile_conversation_memory(config, plugin, debug),
        _ => Ok(()),
    }
}

fn compile_conversation_memory(config: &Value, plugin: &str, debug: bool) -> Result<(), String> {
    let behavior = plugin_memory_behavior(config, plugin);
    let Some(policy) = behavior.compile else {
        return Ok(());
    };
    let recent_values = memory_recent_event_values(
        config,
        plugin,
        policy.event_kind,
        memory_recent_turn_limit(config, plugin, 24),
    )?;
    if recent_values.is_empty() {
        return Ok(());
    }
    let recent_lines = conversation_lines_from_event_values(&recent_values)?;
    if recent_lines.is_empty() {
        return Ok(());
    }
    let existing =
        memory_active_artifact_content(config, plugin, policy.artifact_kind)?.unwrap_or_default();
    let compile_prompt = memory_compile_prompt(
        config,
        plugin,
        "You maintain durable chat memory for noodle.\nRewrite the conversation into a compact factual memory note for future sessions.\nPreserve stable user facts, preferences, recurring goals, active tasks, and unresolved threads.\nPrefer concrete facts over chatter.\nKeep it concise, human-readable, and under 12 short bullet lines.\nDo not mention this instruction.\n",
    );
    let prompt = if existing.trim().is_empty() {
        format!(
            "{}\nConversation to compile:\n{}\n",
            compile_prompt,
            recent_lines.join("\n")
        )
    } else {
        format!(
            "{}\nExisting compiled memory:\n{}\n\nNew conversation to merge:\n{}\n",
            compile_prompt,
            existing,
            recent_lines.join("\n")
        )
    };
    let mut compiled = clean_response_text(&model_output(config, &prompt, debug)?);
    if compiled.trim().is_empty() {
        return Ok(());
    }
    let summary_limit = memory_summary_max_chars(config, plugin, 1600);
    if compiled.len() > summary_limit {
        compiled.truncate(summary_limit);
    }
    let source = json!({
        "event_kind": policy.event_kind,
        "event_count": recent_values.len(),
        "recent_event_ids": recent_values.iter().map(|(id, _)| id).collect::<Vec<_>>(),
    });
    memory_upsert_artifact(config, plugin, policy.artifact_kind, &compiled, &source)?;
    Ok(())
}

pub(crate) fn memory_after_event(
    config: &Value,
    plugin: &str,
    kind: &str,
    increment: i64,
    debug: bool,
) -> Result<(), String> {
    let behavior = plugin_memory_behavior(config, plugin);
    for (event_kind, limit) in behavior.event_limits {
        if event_kind == kind {
            memory_trim_events(config, plugin, kind, limit)?;
        }
    }

    let Some(policy) = behavior.compile else {
        return Ok(());
    };
    if policy.event_kind != kind {
        return Ok(());
    }

    let pending_key = format!("compile.pending.{}", policy.event_kind);
    let mut pending = memory_increment_state_counter(config, plugin, &pending_key)?;
    pending += increment - 1;
    if increment > 1 {
        memory_set_state_counter(config, plugin, &pending_key, pending)?;
    }
    if pending < policy.threshold {
        return Ok(());
    }

    compile_plugin_memory(config, plugin, debug)?;
    memory_set_state_counter(config, plugin, &pending_key, 0)?;
    Ok(())
}

fn memory_recent_conversation_lines(
    config: &Value,
    plugin: &str,
    limit: usize,
) -> Result<Vec<String>, String> {
    let values = memory_recent_event_values(config, plugin, "turn", limit as i64)?;
    conversation_lines_from_event_values(&values)
}

fn memory_plugin_prompt_context(config: &Value, plugin: &str) -> Result<String, String> {
    let summary =
        memory_active_artifact_content(config, plugin, "session_summary")?.unwrap_or_default();
    let recent = memory_recent_conversation_lines(
        config,
        plugin,
        memory_context_turn_limit(config, plugin, 8),
    )?;
    let mut sections = Vec::new();
    if !summary.trim().is_empty() {
        sections.push(format!("Compiled memory:\n{}", summary.trim()));
    }
    if !recent.is_empty() {
        sections.push(format!("Recent conversation:\n{}", recent.join("\n")));
    }
    Ok(sections.join("\n\n"))
}

fn record_turn_memory(
    config: &Value,
    plugin: &str,
    user_text: &str,
    payload: &Value,
    debug: bool,
) -> Result<(), String> {
    let action = actions::DaemonAction::from_value(payload)?;
    let assistant_text = action.primary_text().unwrap_or_default();
    memory_append_event(
        config,
        plugin,
        "turn",
        "user",
        &json!({"role":"user","text": user_text}),
    )?;
    memory_append_event(
        config,
        plugin,
        "turn",
        "assistant",
        &json!({"role":"assistant","text": assistant_text}),
    )?;
    memory_after_event(config, plugin, "turn", 2, debug)?;
    Ok(())
}

fn plugin_include_tool_context(config: &Value, plugin: &str) -> bool {
    let env_name = "NOODLE_CHAT_INCLUDE_TOOL_CONTEXT";
    value_or_env(
        config,
        env_name,
        &format!("plugins.{plugin}.include_tool_context"),
        "0",
    ) != "0"
}

fn resolved_config_path(config: &Value) -> String {
    lookup(config, "_meta.config_path")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            env::var("NOODLE_CONFIG").unwrap_or_else(|_| "~/.noodle/config.json".into())
        })
}

fn external_host_info(config: &Value) -> ExternalHostInfo {
    let module_order = plugin_order(config);
    let slash_commands = tooling::registered_slash_command_definitions(config);
    let tool_counts = module_order
        .iter()
        .map(|plugin| (plugin.clone(), tools_for_plugin(config, plugin).len()))
        .collect::<HashMap<_, _>>();
    let binary_path = env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "noodle".into());
    ExternalHostInfo {
        binary_path: binary_path.clone(),
        module_api: ExternalModuleApiInfo {
            version: MODULE_API_VERSION,
            command_prefix: vec![binary_path.clone(), "module-api".into()],
            capabilities: module_api_capabilities(),
        },
        module_order,
        slash_commands,
        tool_counts,
    }
}

fn spawn_external_plugin(
    manifest: &tooling::PluginManifest,
    config: &Value,
    event: &str,
    input: &str,
    cwd: &str,
    shell: &str,
    exit_status: i64,
    recent_command: &str,
    selected_command: &str,
    debug: bool,
    stream: bool,
) -> Result<std::process::Child, String> {
    let PluginExecution::External {
        manifest_path,
        command,
    } = &manifest.execution
    else {
        return Err(format!("plugin {} is not external", manifest.id));
    };
    let program = command
        .first()
        .ok_or_else(|| format!("external plugin {} has no command", manifest.id))?;
    let request = ExternalPluginRequest {
        api_version: MODULE_API_VERSION,
        event,
        input,
        cwd,
        shell,
        exit_status,
        recent_command,
        selected_command,
        debug,
        stream,
        config_path: resolved_config_path(config),
        host: external_host_info(config),
        config,
    };
    let request_body = serde_json::to_vec(&request).map_err(|err| err.to_string())?;
    let mut child = ProcessCommand::new(program)
        .args(command.iter().skip(1))
        .current_dir(
            Path::new(cwd)
                .is_dir()
                .then_some(cwd)
                .unwrap_or_else(|| manifest_path.parent().and_then(Path::to_str).unwrap_or(".")),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to start external plugin {}: {}", manifest.id, err))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&request_body)
            .map_err(|err| format!("failed to write request to plugin {}: {}", manifest.id, err))?;
    }
    Ok(child)
}

fn invoke_external_plugin(
    manifest: &tooling::PluginManifest,
    config: &Value,
    event: &str,
    input: &str,
    cwd: &str,
    shell: &str,
    exit_status: i64,
    recent_command: &str,
    selected_command: &str,
    debug: bool,
) -> Result<Value, String> {
    let child = spawn_external_plugin(
        manifest,
        config,
        event,
        input,
        cwd,
        shell,
        exit_status,
        recent_command,
        selected_command,
        debug,
        false,
    )?;
    let output = child
        .wait_with_output()
        .map_err(|err| format!("failed waiting for plugin {}: {}", manifest.id, err))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(format!(
            "external plugin {} failed{}",
            manifest.id,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", detail)
            }
        ));
    }
    let response: ExternalPluginResponse = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("invalid response from plugin {}: {}", manifest.id, err))?;
    if response.ok {
        response
            .payload
            .ok_or_else(|| format!("plugin {} returned no payload", manifest.id))
    } else {
        Err(response
            .error
            .unwrap_or_else(|| format!("plugin {} request failed", manifest.id)))
    }
}

fn invoke_external_plugin_streaming(
    manifest: &tooling::PluginManifest,
    config: &Value,
    event: &str,
    input: &str,
    cwd: &str,
    shell: &str,
    exit_status: i64,
    recent_command: &str,
    selected_command: &str,
    debug: bool,
    emitter: &mut dyn FnMut(&actions::DaemonAction) -> Result<(), String>,
) -> Result<Value, String> {
    let mut child = spawn_external_plugin(
        manifest,
        config,
        event,
        input,
        cwd,
        shell,
        exit_status,
        recent_command,
        selected_command,
        debug,
        true,
    )?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("plugin {} has no stdout", manifest.id))?;
    let mut stderr = String::new();
    let mut stderr_pipe = child.stderr.take();
    let reader = io::BufReader::new(stdout);
    let mut final_payload: Option<Value> = None;

    for line in reader.lines() {
        let line = line.map_err(|err| err.to_string())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let raw: Value = serde_json::from_str(trimmed).map_err(|err| {
            format!(
                "invalid streamed response from plugin {}: {}",
                manifest.id, err
            )
        })?;
        if raw.get("type").is_some() {
            let envelope: ExternalPluginStreamEnvelope =
                serde_json::from_value(raw).map_err(|err| {
                    format!(
                        "invalid stream envelope from plugin {}: {}",
                        manifest.id, err
                    )
                })?;
            match envelope.kind.as_str() {
                "action" => {
                    let payload = envelope.payload.ok_or_else(|| {
                        format!("plugin {} action message missing payload", manifest.id)
                    })?;
                    let action = actions::DaemonAction::from_value(&payload)?;
                    emitter(&action)?;
                }
                "final" => {
                    if envelope.ok.unwrap_or(true) {
                        final_payload = envelope.payload;
                    } else {
                        return Err(envelope.error.unwrap_or_else(|| {
                            format!("plugin {} streaming request failed", manifest.id)
                        }));
                    }
                }
                "error" => {
                    return Err(envelope.error.unwrap_or_else(|| {
                        format!("plugin {} streaming request failed", manifest.id)
                    }));
                }
                other => {
                    return Err(format!(
                        "unknown stream message type from plugin {}: {}",
                        manifest.id, other
                    ));
                }
            }
            continue;
        }

        if raw.get("ok").is_some() {
            let response: ExternalPluginResponse = serde_json::from_value(raw)
                .map_err(|err| format!("invalid response from plugin {}: {}", manifest.id, err))?;
            if response.ok {
                final_payload = response.payload;
            } else {
                return Err(response
                    .error
                    .unwrap_or_else(|| format!("plugin {} request failed", manifest.id)));
            }
            continue;
        }

        final_payload = Some(raw);
    }

    if let Some(mut pipe) = stderr_pipe.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let status = child
        .wait()
        .map_err(|err| format!("failed waiting for plugin {}: {}", manifest.id, err))?;
    if !status.success() {
        let detail = stderr.trim().to_string();
        return Err(format!(
            "external plugin {} failed{}",
            manifest.id,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", detail)
            }
        ));
    }
    final_payload.ok_or_else(|| format!("plugin {} returned no payload", manifest.id))
}

fn dispatch_event(
    config: &Value,
    event: &str,
    input: &str,
    cwd: &str,
    shell: &str,
    exit_status: i64,
    recent_command: &str,
    selected_command: &str,
    debug: bool,
) -> Result<Value, String> {
    let manifests = enabled_plugin_manifests(config);
    let plugin_ids = manifests
        .iter()
        .map(|manifest| manifest.id.to_string())
        .collect::<Vec<_>>();
    debug_log(
        debug,
        "dispatch_event",
        &format!("{event} plugins={}", plugin_ids.join(" ")),
    );
    for manifest in manifests {
        if !plugin_matches_request(config, &manifest, event, input) {
            continue;
        }
        match &manifest.execution {
            PluginExecution::External { .. } => {
                debug_log(debug, "matched_plugin", &manifest.id);
                return invoke_external_plugin(
                    &manifest,
                    config,
                    event,
                    input,
                    cwd,
                    shell,
                    exit_status,
                    recent_command,
                    selected_command,
                    debug,
                );
            }
            PluginExecution::Builtin => {}
        }
    }
    Err("no daemon plugin handled event".into())
}

fn dispatch_event_streaming(
    config: &Value,
    event: &str,
    input: &str,
    cwd: &str,
    shell: &str,
    exit_status: i64,
    recent_command: &str,
    selected_command: &str,
    debug: bool,
    emitter: &mut dyn FnMut(&actions::DaemonAction) -> Result<(), String>,
) -> Result<Value, String> {
    let manifests = enabled_plugin_manifests(config);
    for manifest in manifests {
        if !plugin_matches_request(config, &manifest, event, input) {
            continue;
        }
        match &manifest.execution {
            PluginExecution::External { .. } => {
                return invoke_external_plugin_streaming(
                    &manifest,
                    config,
                    event,
                    input,
                    cwd,
                    shell,
                    exit_status,
                    recent_command,
                    selected_command,
                    debug,
                    emitter,
                );
            }
            PluginExecution::Builtin => {}
        }
    }
    Err("no daemon plugin handled event".into())
}

fn workspace_prompt_sections(cwd: &str) -> Vec<String> {
    let cwd_path = expand_home(cwd);
    let workspace_root = discover_workspace_root(&cwd_path);
    let git_root = discover_git_root(&cwd_path);
    let mut sections = Vec::new();
    sections.push(format!("Workspace root: {}", workspace_root.display()));
    sections.push(format!("Current directory: {}", cwd));
    if let Ok(relative) = cwd_path.strip_prefix(&workspace_root) {
        if !relative.as_os_str().is_empty() {
            sections.push(format!(
                "Current directory relative to workspace root: {}",
                relative.display()
            ));
        }
    }
    sections.push(format!(
        "Git repository: {}",
        if git_root.is_some() { "yes" } else { "no" }
    ));
    let entries = fs::read_dir(&workspace_root)
        .ok()
        .into_iter()
        .flat_map(|items| items.flatten())
        .take(16)
        .map(|entry| {
            let mut name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() {
                name.push('/');
            }
            name
        })
        .collect::<Vec<_>>();
    if !entries.is_empty() {
        sections.push(format!(
            "Top-level workspace entries:\n- {}",
            entries.join("\n- ")
        ));
    }
    let broader_roots = broader_local_roots(&workspace_root);
    if !broader_roots.is_empty() {
        sections.push(format!(
            "Broader local roots outside workspace:\n- {}",
            broader_roots.join("\n- ")
        ));
    }
    sections
}

fn broader_local_roots(workspace_root: &Path) -> Vec<String> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("~"));
    broader_local_roots_from_home(&home, workspace_root)
}

fn broader_local_roots_from_home(home: &Path, workspace_root: &Path) -> Vec<String> {
    let candidates = ["Dropbox", "Desktop", "Documents", "Downloads", "Sites"]
        .into_iter()
        .map(|name| home.join(name))
        .collect::<Vec<_>>();
    candidates
        .into_iter()
        .filter(|path| path.exists())
        .filter(|path| path != workspace_root)
        .filter(|path| !path.starts_with(workspace_root))
        .map(|path| path.display().to_string())
        .collect()
}

fn discover_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn discover_workspace_root(start: &Path) -> PathBuf {
    let mut current = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent().unwrap_or(start).to_path_buf()
    };
    loop {
        if current.join(".git").exists()
            || current.join("Cargo.toml").exists()
            || current.join("package.json").exists()
            || current.join("pyproject.toml").exists()
        {
            return current;
        }
        if !current.pop() {
            return start.to_path_buf();
        }
    }
}

fn model_output(config: &Value, prompt: &str, debug: bool) -> Result<String, String> {
    match value_or_env(config, "NOODLE_PROVIDER", "provider", "openai_responses")
        .to_lowercase()
        .as_str()
    {
        "stub" => call_stub_model(config, prompt),
        "openai_responses" => call_openai_responses(config, prompt, debug),
        "openai_compatible" => call_openai_compatible(config, prompt, debug),
        "anthropic" => call_anthropic(config, prompt, debug),
        provider => Err(format!("unsupported provider: {provider}")),
    }
}

fn call_stub_model(config: &Value, prompt: &str) -> Result<String, String> {
    if let Some(matchers) = lookup(config, "stub.matchers").and_then(Value::as_array) {
        for matcher in matchers {
            let contains = matcher
                .get("contains")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let response = matcher
                .get("response")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !contains.is_empty() && prompt.contains(contains) {
                return Ok(response.to_string());
            }
        }
    }
    if value_or_env(config, "", "stub.mode", "").eq_ignore_ascii_case("selector") {
        if let Some(response) = call_selector_stub(prompt) {
            return Ok(response);
        }
    }
    Ok(value_or_env(config, "", "stub.default_response", "stub-ok"))
}

fn call_selector_stub(prompt: &str) -> Option<String> {
    let task_directive = prompt_section(prompt, "Task Directive").unwrap_or_default();
    if task_directive.contains("Task finished. Respond with FINAL only.") {
        if task_directive.contains("interactive harness session") {
            return Some("FINAL: interactive shell completed.".into());
        }
        return Some("FINAL: completed".into());
    }
    if task_directive.contains("You previously drafted a direct answer before using any tools.") {
        return Some("FINAL_OK".into());
    }
    if prompt.contains("hi:alex") {
        return Some("FINAL: interactive shell completed.".into());
    }
    if prompt.contains("name?") {
        return Some(
            r#"TOOL: interactive_shell_write {"session_id":"__TOOL_RESULT_0__.output.session_id","text":"alex","submit":true}"#
                .into(),
        );
    }

    let request = prompt_section(prompt, "Current Request")?;
    let lowered = request.to_lowercase();

    let response = if lowered.contains("find me the readme file in this repo") {
        r#"PLAN: locate readme in repo
STEP: path_search {"root":".","query":"README","kind":"file","limit":10}"#
    } else if lowered.contains("show me the contents of readme.md in this directory") {
        r#"PLAN: read local readme
STEP: file_read {"path":"README.md"}"#
    } else if lowered.contains("list every txt file under the harness folder") {
        r#"PLAN: list harness text files
STEP: glob {"root":"harness","pattern":"*.txt","limit":20}"#
    } else if lowered.contains("search the harness folder for the word needle") {
        r#"PLAN: grep harness for needle
STEP: grep {"root":"harness","pattern":"needle","limit":20}"#
    } else if lowered.contains("fetch https://example.test/page") {
        r#"PLAN: fetch example page
STEP: web_fetch {"url":"https://example.test/page","_stub":{"web_fetch":{"https://example.test/page":"fetched-page"}}}"#
    } else if lowered.contains("search the web for rust sqlite") {
        r#"PLAN: web search rust sqlite
STEP: web_search {"query":"rust sqlite","_stub":{"web_search":{"rust sqlite":[{"title":"Result One","url":"https://example.test/1"},{"title":"Result Two","url":"https://example.test/2"}]}}}"#
    } else if lowered.contains("write a file named written-by-harness.txt in the harness folder") {
        r#"PLAN: write harness file
STEP: file_write {"path":"harness/written-by-harness.txt","content":"written by harness"}"#
    } else if lowered.contains("replace before with after in harness/edit-target.txt") {
        r#"PLAN: edit harness file
STEP: file_edit {"path":"harness/edit-target.txt","find":"before","replace":"after"}"#
    } else if lowered.contains("run printf harness-shell-ok in the harness folder") {
        r#"PLAN: run harness shell command
STEP: shell_exec {"command":"printf harness-shell-ok","cwd":"harness"}"#
    } else if lowered.contains("talk to the interactive harness and tell it alex") {
        r#"PLAN: interactive harness session
STEP: interactive_shell_start {"command":"printf 'name? '; read name; printf 'hi:%s' $name"}"#
    } else if lowered.contains("list the mcp tools on docs") {
        r#"PLAN: list mcp tools
STEP: mcp_tools_list {"server":"docs"}"#
    } else if lowered.contains("call the echo tool on docs with hello") {
        r#"PLAN: call mcp echo tool
STEP: mcp_tool_call {"server":"docs","tool":"echo","arguments":{"text":"hello"}}"#
    } else if lowered.contains("read the mcp memory summary resource from docs") {
        r#"PLAN: read mcp resource
STEP: mcp_resource_read {"server":"docs","uri":"memory://summary"}"#
    } else if lowered.contains("save a task note called harness_note saying ship noodle") {
        r#"PLAN: write task note
STEP: task_note_write {"kind":"harness_note","content":"ship noodle"}"#
    } else if lowered.contains("create an agent handoff for planner saying remember this") {
        r#"PLAN: create agent handoff
STEP: agent_handoff_create {"agent":"planner","content":"remember this"}"#
    } else if lowered.contains("show me the current task artifacts in noodle memory") {
        r#"PLAN: query task artifacts
STEP: memory_query {"source":"artifacts","limit":10}"#
    } else {
        return None;
    };

    Some(response.into())
}

fn prompt_section(prompt: &str, title: &str) -> Option<String> {
    let marker = format!("[{title}]\n");
    let start = prompt.find(&marker)? + marker.len();
    let rest = &prompt[start..];
    let end = rest.find("\n\n[").unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

fn http_client(timeout_seconds: u64) -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|err| err.to_string())
}

fn call_openai_compatible(config: &Value, prompt: &str, debug: bool) -> Result<String, String> {
    let base_url = value_or_env(config, "NOODLE_BASE_URL", "base_url", "");
    let model = value_or_env(config, "NOODLE_MODEL", "model", "");
    if base_url.is_empty() || model.is_empty() {
        return Err("openai_compatible requires base_url and model".into());
    }
    let api_key = sanitized_header_value(&value_or_env(config, "NOODLE_API_KEY", "api_key", ""));
    let timeout = value_or_env(config, "NOODLE_TIMEOUT_SECONDS", "timeout_seconds", "20")
        .parse::<u64>()
        .unwrap_or(20);
    let mut max_tokens = configured_max_tokens(config);
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    for attempt in 0..3 {
        let payload = json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "max_completion_tokens": max_tokens
        });
        let body = request_json(&url, api_key.as_str(), &payload, timeout, debug)?;
        let text = extract_openai_compatible_text(&body);
        if !openai_compatible_response_was_truncated(&body) {
            return text.ok_or_else(|| "empty model response".into());
        }
        if attempt == 2 {
            return text.ok_or_else(|| {
                "model response was truncated by max_completion_tokens and no text was returned"
                    .into()
            });
        }
        max_tokens = expanded_max_tokens(max_tokens);
    }

    Err("failed to obtain a complete model response".into())
}

fn call_openai_responses(config: &Value, prompt: &str, debug: bool) -> Result<String, String> {
    let base_url = value_or_env(
        config,
        "NOODLE_BASE_URL",
        "base_url",
        "https://api.openai.com/v1",
    );
    let model = value_or_env(config, "NOODLE_MODEL", "model", "");
    let api_key = sanitized_header_value(&value_or_env(config, "NOODLE_API_KEY", "api_key", ""));
    if model.is_empty() || api_key.is_empty() {
        return Err("openai_responses requires model and api_key".into());
    }
    let timeout = value_or_env(config, "NOODLE_TIMEOUT_SECONDS", "timeout_seconds", "20")
        .parse::<u64>()
        .unwrap_or(20);
    let mut max_tokens = configured_max_tokens(config);
    let reasoning_effort = value_or_env(config, "NOODLE_REASONING_EFFORT", "reasoning_effort", "");
    let url = format!("{}/responses", base_url.trim_end_matches('/'));

    for attempt in 0..3 {
        let mut payload = json!({
            "model": model,
            "input": prompt,
            "max_output_tokens": max_tokens,
        });
        if !reasoning_effort.is_empty() {
            payload["reasoning"] = json!({ "effort": reasoning_effort });
        }

        let body = request_json(&url, api_key.as_str(), &payload, timeout, debug)?;
        let text = extract_openai_responses_text(&body);
        if !openai_responses_body_was_truncated(&body) {
            if text.trim().is_empty() {
                return Err(format!(
                    "empty response body from OpenAI responses API: {body}"
                ));
            }
            return Ok(text);
        }
        if attempt == 2 {
            if !text.trim().is_empty() {
                return Ok(text);
            }
            return Err(format!(
                "response was truncated by max_output_tokens before any visible text was returned: {body}"
            ));
        }
        max_tokens = expanded_max_tokens(max_tokens);
    }

    Err("failed to obtain a complete model response".into())
}

const DEFAULT_MAX_TOKENS: u64 = 1024;
const MAX_RETRY_MAX_TOKENS: u64 = 2048;

fn configured_max_tokens(config: &Value) -> u64 {
    value_or_env(
        config,
        "NOODLE_MAX_TOKENS",
        "max_tokens",
        &DEFAULT_MAX_TOKENS.to_string(),
    )
    .parse::<u64>()
    .unwrap_or(DEFAULT_MAX_TOKENS)
}

fn expanded_max_tokens(current: u64) -> u64 {
    current
        .saturating_mul(2)
        .max(DEFAULT_MAX_TOKENS)
        .min(MAX_RETRY_MAX_TOKENS)
}

fn extract_openai_compatible_text(body: &Value) -> Option<String> {
    body.pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|text| !text.trim().is_empty())
}

fn openai_compatible_response_was_truncated(body: &Value) -> bool {
    body.pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        == Some("length")
}

fn extract_openai_responses_text(body: &Value) -> String {
    if let Some(text) = body.get("output_text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return text.to_owned();
        }
    }

    let mut pieces = Vec::new();
    if let Some(items) = body.get("output").and_then(Value::as_array) {
        for item in items {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for part in content {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            pieces.push(text.to_owned());
                        }
                    } else if let Some(text) = part.get("output_text").and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            pieces.push(text.to_owned());
                        }
                    }
                }
            }
        }
    }
    pieces.join("")
}

fn openai_responses_body_was_truncated(body: &Value) -> bool {
    body.get("status").and_then(Value::as_str) == Some("incomplete")
        && body
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
            == Some("max_output_tokens")
}

fn call_anthropic(config: &Value, prompt: &str, debug: bool) -> Result<String, String> {
    let base_url = value_or_env(
        config,
        "NOODLE_BASE_URL",
        "base_url",
        "https://api.anthropic.com/v1",
    );
    let model = value_or_env(config, "NOODLE_MODEL", "model", "");
    let api_key = sanitized_header_value(&value_or_env(config, "NOODLE_API_KEY", "api_key", ""));
    if model.is_empty() || api_key.is_empty() {
        return Err("anthropic requires model and api_key".into());
    }
    let timeout = value_or_env(config, "NOODLE_TIMEOUT_SECONDS", "timeout_seconds", "20")
        .parse::<u64>()
        .unwrap_or(20);
    let max_tokens = configured_max_tokens(config);
    let payload = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{"role": "user", "content": prompt}]
    });

    let body = request_json_with_headers(
        &format!("{}/messages", base_url.trim_end_matches('/')),
        vec![
            ("x-api-key", api_key.as_str()),
            ("anthropic-version", "2023-06-01"),
        ],
        &payload,
        timeout,
        debug,
    )?;

    let mut pieces = Vec::new();
    if let Some(items) = body.get("content").and_then(Value::as_array) {
        for item in items {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                pieces.push(text.to_owned());
            }
        }
    }
    if pieces.is_empty() {
        return Err("empty model response".into());
    }
    Ok(pieces.join(""))
}

fn request_json(
    url: &str,
    api_key: &str,
    payload: &Value,
    timeout_seconds: u64,
    debug: bool,
) -> Result<Value, String> {
    let mut headers = Vec::new();
    if !api_key.is_empty() {
        headers.push((AUTHORIZATION.as_str(), format!("Bearer {api_key}")));
    }
    request_json_with_headers(
        url,
        headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect(),
        payload,
        timeout_seconds,
        debug,
    )
}

fn request_json_with_headers(
    url: &str,
    extra_headers: Vec<(&str, &str)>,
    payload: &Value,
    timeout_seconds: u64,
    debug: bool,
) -> Result<Value, String> {
    debug_log(debug, "request_url", url);
    debug_log(debug, "request_payload", &payload.to_string());
    let client = http_client(timeout_seconds)?;
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    for (name, value) in extra_headers {
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| err.to_string())?;
        let sanitized_value = sanitized_header_value(value);
        let header_value = HeaderValue::from_str(&sanitized_value).map_err(|err| {
            format!("invalid value for header {name}: {err}")
        })?;
        headers.insert(header_name, header_value);
    }
    let response = client
        .post(url)
        .headers(headers)
        .body(payload.to_string())
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let body = response.text().map_err(|err| err.to_string())?;
    debug_log(debug, "response_body", &body);
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status.as_u16(), body));
    }
    serde_json::from_str::<Value>(&body).map_err(|err| err.to_string())
}

fn clean_response_text(text: &str) -> String {
    let mut cleaned = text.trim().replace("```json", "").replace("```", "");
    for pattern in ["<think>", "</think>", "<thinking>", "</thinking>"] {
        cleaned = cleaned.replace(pattern, "");
    }
    cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let lower = line.to_lowercase();
            !["json:", "command:", "answer:"].contains(&lower.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_runtime_config(config: &Value) -> String {
    let slash_commands = registered_slash_command_names(config).join(" ");
    let plugin_order_text = plugin_order(config).join(" ");
    format!(
        "debug={}\nauto_run={}\nenable_error_fallback={}\nmax_retry_depth={}\nplugin_order={}\nselection_mode={}\nslash_commands={}\nchat_prefix={}\n",
        value_or_env(config, "NOODLE_DEBUG", "runtime.debug", "0"),
        value_or_env(config, "NOODLE_AUTO_RUN", "runtime.auto_run", "1"),
        value_or_env(
            config,
            "NOODLE_ENABLE_ERROR_FALLBACK",
            "runtime.enable_error_fallback",
            "0",
        ),
        value_or_env(
            config,
            "NOODLE_MAX_RETRY_DEPTH",
            "runtime.max_retry_depth",
            "2"
        ),
        env::var("NOODLE_PLUGIN_ORDER").unwrap_or(plugin_order_text),
        value_or_env(
            config,
            "NOODLE_SELECTION_MODE",
            "plugins.typos.selection_mode",
            "select",
        ),
        slash_commands,
        value_or_env(config, "NOODLE_CHAT_PREFIX", "plugins.chat.prefix", ","),
    )
}

fn render_payload_fields(payload: &str) -> Result<String, String> {
    let parsed = serde_json::from_str::<Value>(payload).map_err(|err| err.to_string())?;
    let mut output = String::new();
    if let Some(action) = parsed.get("action").and_then(Value::as_str) {
        output.push_str(&format!("action={action}\n"));
    }
    if let Some(plugin) = parsed.get("plugin").and_then(Value::as_str) {
        output.push_str(&format!("plugin={}\n", encode_field(plugin)));
    }
    for field in ["command", "question", "explanation", "message", "text"] {
        if let Some(value) = parsed.get(field).and_then(Value::as_str) {
            output.push_str(&format!("{field}={}\n", encode_field(value)));
        }
    }
    for field in [
        "permission_id",
        "task_id",
        "tool",
        "permission_class",
        "summary",
        "status",
    ] {
        if let Some(value) = parsed.get(field).and_then(Value::as_str) {
            output.push_str(&format!("{field}={}\n", encode_field(value)));
        }
    }
    for field in ["index", "total"] {
        if let Some(value) = parsed.get(field).and_then(Value::as_u64) {
            output.push_str(&format!("{field}={value}\n"));
        }
    }
    if let Some(choices) = parsed.get("choices").and_then(Value::as_array) {
        for choice in choices {
            if let Some(choice) = choice.as_str() {
                output.push_str(&format!("choice={}\n", encode_field(choice)));
            }
        }
    }
    if let Some(items) = parsed.get("items").and_then(Value::as_array) {
        for item in items {
            output.push_str(&format!(
                "item={}\n",
                encode_field(&serde_json::to_string(item).map_err(|err| err.to_string())?)
            ));
        }
    }
    Ok(output)
}

fn tool_json(tool: ToolDefinition) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "tier": tool.tier.as_str(),
        "permission": tool.permission.as_str(),
        "input_schema": tool.input_schema,
    })
}

fn debug_log(debug: bool, label: &str, value: &str) {
    if debug {
        eprintln!("[noodle] {label}: {value}");
    }
}

fn encode_field(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        encoded.push_str(&format!("{:02x}", byte));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        broader_local_roots_from_home, call_selector_stub, expanded_max_tokens,
        openai_compatible_response_was_truncated, openai_responses_body_was_truncated,
        strip_terminal_control_sequences,
    };
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), nanos));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn broader_local_roots_lists_existing_home_roots_outside_workspace() {
        let temp = TempDir::new("noodle-main-broader-roots");
        let home = temp.path().join("home");
        let workspace = home.join("Sites/noodle");
        fs::create_dir_all(home.join("Dropbox")).unwrap();
        fs::create_dir_all(home.join("Documents")).unwrap();
        fs::create_dir_all(&workspace).unwrap();

        let roots = broader_local_roots_from_home(&home, &workspace);
        assert!(roots.contains(&home.join("Dropbox").display().to_string()));
        assert!(roots.contains(&home.join("Documents").display().to_string()));
        assert!(!roots.contains(&workspace.display().to_string()));
    }

    #[test]
    fn selector_starts_interactive_shell_for_interactive_request() {
        let prompt =
            "[Current Request]\ntalk to the interactive harness and tell it alex".to_string();
        let response = call_selector_stub(&prompt).unwrap();
        assert!(response.starts_with("PLAN: interactive harness session"));
        assert!(response.contains("STEP: interactive_shell_start "));
    }

    #[test]
    fn selector_finalizes_interactive_harness_tasks() {
        let prompt = format!(
            "[Task Directive]\nPlanned task summary: interactive harness session\nTask finished. Respond with FINAL only.\n\n[Tool Results]\n{}",
            serde_json::to_string(&json!({"ok":true})).unwrap()
        );
        let response = call_selector_stub(&prompt).unwrap();
        assert_eq!(response, "FINAL: interactive shell completed.");
    }

    #[test]
    fn responses_api_truncation_is_detected() {
        let body = json!({
            "status": "incomplete",
            "incomplete_details": {
                "reason": "max_output_tokens"
            }
        });
        assert!(openai_responses_body_was_truncated(&body));
    }

    #[test]
    fn chat_completions_truncation_is_detected() {
        let body = json!({
            "choices": [
                {
                    "finish_reason": "length"
                }
            ]
        });
        assert!(openai_compatible_response_was_truncated(&body));
    }

    #[test]
    fn max_token_retry_grows_to_useful_sizes() {
        assert_eq!(expanded_max_tokens(160), 1024);
        assert_eq!(expanded_max_tokens(512), 1024);
        assert_eq!(expanded_max_tokens(1024), 2048);
        assert_eq!(expanded_max_tokens(2048), 2048);
    }

    #[test]
    fn header_sanitizer_removes_terminal_escape_sequences() {
        assert_eq!(
            strip_terminal_control_sequences("\u{1b}[O\u{1b}[Isk-proj-test\n"),
            "sk-proj-test"
        );
        assert_eq!(
            strip_terminal_control_sequences("\u{1b}sk-proj-test"),
            "sk-proj-test"
        );
    }
}
