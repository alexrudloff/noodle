#![recursion_limit = "512"]

use serde_json::Value;
use serde_json::json;
use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn noodle_bin() -> &'static str {
    env!("CARGO_BIN_EXE_noodle")
}

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

fn write_mock_mcp_server(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("mock_mcp_server.py");
    fs::write(
        &path,
        r#"#!/usr/bin/env python3
import json
import sys

counter = 0
mode = sys.argv[1] if len(sys.argv) > 1 else "content_length"


def read_message():
    if mode == "ndjson":
        while True:
            line = sys.stdin.buffer.readline()
            if not line:
                return None
            line = line.decode("utf-8").strip()
            if line:
                return json.loads(line)
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("utf-8").split(":", 1)
        headers[name.strip().lower()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = sys.stdin.buffer.read(length)
    if not body:
        return None
    return json.loads(body.decode("utf-8"))


def send(message):
    if mode == "ndjson":
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()
        return
    body = json.dumps(message).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()


def ok(request_id, result):
    send({"jsonrpc": "2.0", "id": request_id, "result": result})


def error(request_id, code, message):
    send(
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": code, "message": message},
        }
    )


while True:
    request = read_message()
    if request is None:
        break
    method = request.get("method")
    request_id = request.get("id")
    params = request.get("params") or {}

    if method == "initialize":
        ok(
            request_id,
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {
                    "tools": {"listChanged": False},
                    "resources": {"listChanged": False},
                },
                "serverInfo": {"name": "mock-docs", "version": "1.0.0"},
            },
        )
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        ok(
            request_id,
            {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo text back from the MCP server.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"],
                            "additionalProperties": False,
                        },
                    },
                    {
                        "name": "counter",
                        "description": "Return an incrementing counter.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "additionalProperties": False,
                        },
                    },
                ]
            },
        )
    elif method == "tools/call":
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "echo":
            text = arguments.get("text", "")
            ok(
                request_id,
                {
                    "content": [{"type": "text", "text": f"echo:{text}"}],
                    "isError": False,
                },
            )
        elif name == "counter":
            counter += 1
            ok(
                request_id,
                {
                    "content": [{"type": "text", "text": f"counter:{counter}"}],
                    "isError": False,
                },
            )
        else:
            error(request_id, -32601, f"unknown tool: {name}")
    elif method == "resources/list":
        ok(
            request_id,
            {
                "resources": [
                    {
                        "uri": "memory://summary",
                        "name": "Summary",
                        "mimeType": "text/plain",
                    }
                ]
            },
        )
    elif method == "resources/read":
        uri = params.get("uri")
        if uri == "memory://summary":
            ok(
                request_id,
                {
                    "contents": [
                        {
                            "uri": uri,
                            "mimeType": "text/plain",
                            "text": "memory summary",
                        }
                    ]
                },
            )
        else:
            error(request_id, -32001, f"unknown resource: {uri}")
    else:
        if request_id is not None:
            error(request_id, -32601, f"method not found: {method}")
"#,
    )
    .unwrap();
    path
}

fn write_test_config(temp: &TempDir, selection_mode: &str) -> PathBuf {
    let config_path = temp.path().join("config.json");
    let memory_path = temp.path().join("memory.db");
    let mock_mcp_server = write_mock_mcp_server(temp);
    let modules_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("modules")
        .display()
        .to_string();
    let config = json!({
        "provider": "stub",
        "model": "gpt-5",
        "max_tokens": 160,
        "reasoning_effort": "minimal",
        "stub": {
            "mode": "selector",
            "default_response": "generic-ok",
            "matchers": [
                {
                    "contains": "[Task Directive]\nYou previously drafted a direct answer before using any tools.\nDraft answer:\nNo files found. There isn’t a claude-code-main directory (or its README) under the workspace. Want me to search the whole disk or fetch from a remote source if you have a link?",
                    "response": "STEP: glob {\"root\": \".\", \"pattern\": \"claude-code-main*README.md\"}"
                },
                {
                    "contains": "TOOL_RESULT: glob",
                    "response": "FINAL: __FOUND_README__"
                },
                {
                    "contains": "[Task Directive]\nYou previously drafted a direct answer before using any tools.",
                    "response": "FINAL_OK"
                },
                {
                    "contains": "[Current Request]\nhello",
                    "response": "chat-ok"
                },
                {
                    "contains": "[Current Request]\ntest",
                    "response": "prefix-ok"
                },
                {
                    "contains": "[Current Request]\ninspect workspace context",
                    "response": "workspace-aware-ok"
                },
                {
                    "contains": "[Current Request]\nwhat does readme.md say?",
                    "response": "STEP: file_read {\"path\":\"README.md\"}"
                },
                {
                    "contains": "[Current Request]\nfind the readme file in claude-code-main",
                    "response": "FINAL: No files found. There isn’t a claude-code-main directory (or its README) under the workspace. Want me to search the whole disk or fetch from a remote source if you have a link?"
                },
                {
                    "contains": "TOOL_RESULT: file_read {\"path\":\"__README_FILE__\"}",
                    "response": "FINAL: concise-readme-summary"
                },
                {
                    "contains": "[Current Request]\nsummarize readme.md briefly",
                    "response": "STEP: file_read {\"path\":\"README.md\"}"
                },
                {
                    "contains": "Planned task summary: inspect then summarize",
                    "response": "FINAL: I inspected both files and summarized them."
                },
                {
                    "contains": "Planned task summary: fallback after failure",
                    "response": "FINAL: I recovered after the failed step."
                },
                {
                    "contains": "Planned task summary: write then confirm",
                    "response": "FINAL: I wrote the file successfully."
                },
                {
                    "contains": "[Current Request]\ninspect the workspace deeply",
                    "response": "PLAN: inspect then summarize\nSTEP: file_read {\"path\":\"__PLAN_FILE_ONE__\"}\nSTEP: file_read {\"path\":\"__PLAN_FILE_TWO__\"}"
                },
                {
                    "contains": "[Current Request]\nread the chained file",
                    "response": "TOOL: file_read {\"path\":\"__CHAINED_FILE__\"}"
                },
                {
                    "contains": "Input: die",
                    "response": "echo typo-ok\nls\npwd"
                },
                {
                    "contains": "TOOL_RESULT: shell_exec",
                    "response": "FINAL: shell execution completed."
                },
                {
                    "contains": "[Current Request]\nrun the shell task",
                    "response": "TOOL: shell_exec {\"command\":\"printf permission-shell-ok\"}"
                },
                {
                    "contains": "[Current Request]\nwrite the planned file",
                    "response": "PLAN: write then confirm\nSTEP: file_write {\"path\":\"__WRITE_FILE__\",\"content\":\"written-by-plan\"}"
                },
                {
                    "contains": "Original goal: recover from a broken plan",
                    "response": "PLAN: fallback after failure\nSTEP: file_read {\"path\":\"__CHAINED_FILE__\"}"
                },
                {
                    "contains": "[Current Request]\nrecover from a broken plan",
                    "response": "PLAN: broken first step\nSTEP: file_read {\"path\":\"__MISSING_FILE__\"}"
                },
                {
                    "contains": "durable chat memory",
                    "response": "- user said hello\n- assistant replied chat-ok"
                }
            ]
        },
        "soul": "You are noodle, a concise zsh assistant.",
        "runtime": {
            "debug": 0,
            "max_retry_depth": 2,
            "auto_run": 1,
            "enable_error_fallback": 0
        },
        "search": {
            "provider": "duckduckgo_html",
            "brave": {
                "api_key": "",
                "base_url": "https://api.search.brave.com/res/v1/web/search",
                "country": "us",
                "search_lang": "en"
            }
        },
        "mcp": {
            "servers": {
                "docs": {
                    "command": ["python3", mock_mcp_server.display().to_string()],
                    "request_timeout_ms": 5000,
                    "startup_timeout_ms": 5000
                },
                "docs_ndjson": {
                    "command": ["python3", mock_mcp_server.display().to_string(), "ndjson"],
                    "message_format": "ndjson",
                    "request_timeout_ms": 5000,
                    "startup_timeout_ms": 5000
                }
            }
        },
        "memory": {
            "path": memory_path.to_string_lossy().to_string(),
            "chat": {
                "recent_turn_limit": 24,
                "context_turn_limit": 8,
                "summary_max_chars": 1600,
                "compile_after_events": 100,
                "compile_prompt": "You maintain durable chat memory for noodle."
            },
            "typos": {
                "context_limit": 3,
                "selection_event_limit": 200
            },
            "todo": {
                "command_event_limit": 200
            }
        },
        "modules": {
            "paths": [modules_path]
        },
        "plugins": {
            "order": ["utils", "memory", "scripting", "todo", "chat", "typos"],
            "utils": {
                "uses_tools": [],
                "tool_availability": {},
                "exports_tools": []
            },
            "memory": {
                "uses_tools": [],
                "tool_availability": {},
                "exports_tools": []
            },
            "scripting": {
                "uses_tools": [],
                "tool_availability": {},
                "exports_tools": []
            },
            "todo": {
                "uses_tools": [],
                "tool_availability": {},
                "exports_tools": []
            },
            "chat": {
                "prefix": ",",
                "include_tool_context": 0,
                "tool_calling": 1,
                "task_execution": 1,
                "max_tool_rounds": 8,
                "max_replans": 1,
                "uses_tools": [
                    "memory_query",
                    "file_read",
                    "path_search",
                    "glob",
                    "grep",
                    "web_fetch",
                    "web_search",
                    "file_write",
                    "file_edit",
                    "shell_exec",
                    "interactive_shell_start",
                    "interactive_shell_read",
                    "interactive_shell_write",
                    "interactive_shell_key",
                    "interactive_shell_close",
                    "mcp_tools_list",
                    "mcp_tool_call",
                    "mcp_resources_list",
                    "mcp_resource_read",
                    "task_note_write",
                    "agent_handoff_create"
                ],
                "exports_tools": ["chat.send"],
                "prompt": "You are noodle, a local terminal agent. You help the user think, search, inspect files, read and edit code, run commands, and complete tasks using the tools available to you. You are workspace-aware when relevant, but not limited to software engineering or zsh. Be concise, practical, and action-oriented."
            },
            "typos": {
                "selection_mode": selection_mode,
                "uses_tools": [],
                "exports_tools": [],
                "prompt": "You are a zsh typo fixer.\nThe user typed a mistaken command and zsh could not find it.\nReturn exactly 3 lines.\nEach line must contain only one command the user most likely intended to run in zsh.\nPrefer common shell commands over obscure executables.\nPrefer the intended zsh command, not merely the nearest executable name.\nNo numbering.\nNo explanation.\nNo extra text.\nInput: {user_input}\n"
            }
        }
    });
    let chained_file = temp.path().join("chained.txt");
    let readme_file = temp.path().join("README.md");
    let project_dir = temp.path().join("claude-code-main");
    let found_readme = project_dir.join("README.md");
    let harness_root = temp.path().join("harness");
    let harness_file = harness_root.join("alpha.txt");
    let harness_edit_file = harness_root.join("edit-target.txt");
    let harness_village_dir = harness_root.join("Tobacco Branch Village POA");
    let plan_file_one = temp.path().join("plan-one.txt");
    let plan_file_two = temp.path().join("plan-two.txt");
    let write_file = temp.path().join("write-plan.txt");
    let missing_file = temp.path().join("missing.txt");
    fs::write(&chained_file, "tool-chain-ok").unwrap();
    fs::write(&readme_file, "relative-readme-ok").unwrap();
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(&found_readme, "found-project-readme").unwrap();
    fs::create_dir_all(&harness_village_dir).unwrap();
    fs::write(&harness_file, "alpha\nneedle here\n").unwrap();
    fs::write(harness_village_dir.join("agenda.txt"), "meeting notes").unwrap();
    fs::write(&harness_edit_file, "before").unwrap();
    fs::write(&plan_file_one, "plan-one").unwrap();
    fs::write(&plan_file_two, "plan-two").unwrap();
    let mut config_body = serde_json::to_string_pretty(&config).unwrap();
    config_body = config_body.replace("__CHAINED_FILE__", &chained_file.display().to_string());
    config_body = config_body.replace("__README_FILE__", &readme_file.display().to_string());
    config_body = config_body.replace("__FOUND_README__", &found_readme.display().to_string());
    config_body = config_body.replace("__PLAN_FILE_ONE__", &plan_file_one.display().to_string());
    config_body = config_body.replace("__PLAN_FILE_TWO__", &plan_file_two.display().to_string());
    config_body = config_body.replace("__WRITE_FILE__", &write_file.display().to_string());
    config_body = config_body.replace("__MISSING_FILE__", &missing_file.display().to_string());
    fs::write(&config_path, config_body).unwrap();
    config_path
}

fn run_zsh(temp: &TempDir, config: &Path, script: &str) -> String {
    strip_ansi(&run_zsh_raw(temp, config, script))
}

fn run_zsh_raw(temp: &TempDir, config: &Path, script: &str) -> String {
    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");
    let output = Command::new("/bin/zsh")
        .arg("-ic")
        .arg(format!(
            "source '{}'; {}",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugin/noodle.plugin.zsh")
                .display(),
            script
        ))
        .env("HOME", temp.path())
        .env("NOODLE_HELPER", noodle_bin())
        .env("NOODLE_CONFIG", config)
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1")
        .output()
        .unwrap();
    kill_daemon(&pidfile, &socket);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    combined
}

fn kill_daemon(pidfile: &Path, socket: &Path) {
    if let Ok(pid) = fs::read_to_string(pidfile) {
        let _ = Command::new("kill").arg(pid.trim()).status();
    }
    let _ = fs::remove_file(pidfile);
    let _ = fs::remove_file(socket);
}

fn run_tool(
    temp: &TempDir,
    config: &Path,
    tool: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");
    let output = Command::new(noodle_bin())
        .arg("tool-call")
        .arg("--config")
        .arg(config)
        .arg("--tool")
        .arg(tool)
        .arg("--args")
        .arg(serde_json::to_string(&args).unwrap())
        .env("HOME", temp.path())
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tool {} failed: {}\n{}",
        tool,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn run_tool_batch(temp: &TempDir, config: &Path, calls: serde_json::Value) -> serde_json::Value {
    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");
    let output = Command::new(noodle_bin())
        .arg("tool-batch")
        .arg("--config")
        .arg(config)
        .arg("--calls")
        .arg(serde_json::to_string(&calls).unwrap())
        .env("HOME", temp.path())
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tool batch failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn run_mode(
    temp: &TempDir,
    config: &Path,
    mode: &str,
    input: &str,
    selected_command: Option<&str>,
) -> serde_json::Value {
    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");
    let mut command = Command::new(noodle_bin());
    command
        .arg("--mode")
        .arg(mode)
        .arg("--input")
        .arg(input)
        .arg("--cwd")
        .arg(temp.path())
        .arg("--shell")
        .arg("zsh")
        .arg("--exit-status")
        .arg("127")
        .arg("--recent-command")
        .arg("")
        .arg("--config")
        .arg(config)
        .env("HOME", temp.path())
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1");
    if let Some(selected) = selected_command {
        command.arg("--selected-command").arg(selected);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "mode {} failed: {}\n{}",
        mode,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn run_client_failure(temp: &TempDir, config: &Path, args: &[&str]) -> String {
    let socket = temp.path().join("missing.sock");
    let pidfile = temp.path().join("missing.pid");
    let output = Command::new(noodle_bin())
        .args(args)
        .env("HOME", temp.path())
        .env("NOODLE_CONFIG", config)
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .output()
        .unwrap();
    assert!(!output.status.success(), "client unexpectedly succeeded");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run_task_command(
    temp: &TempDir,
    config: &Path,
    subcommand: &str,
    extra_args: &[&str],
) -> serde_json::Value {
    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");
    let mut command = Command::new(noodle_bin());
    command
        .arg(subcommand)
        .arg("--config")
        .arg(config)
        .env("HOME", temp.path())
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1");
    for arg in extra_args {
        command.arg(arg);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{} failed: {}\n{}",
        subcommand,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn batch_items<'a>(value: &'a serde_json::Value) -> &'a [serde_json::Value] {
    value["items"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn last_batch_action<'a>(value: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    batch_items(value).last()
}

fn task_id_from_batch(value: &serde_json::Value) -> String {
    batch_items(value)
        .iter()
        .find(|item| item["action"].as_str() == Some("task_started"))
        .and_then(|item| item["task_id"].as_str())
        .unwrap()
        .to_string()
}

fn task_step_from_show<'a>(value: &'a serde_json::Value) -> &'a serde_json::Value {
    &value["task"]["steps"][0]
}

fn set_config_permissions_allow(config: &Path) {
    let mut value: Value = serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();
    value["permissions"]["classes"]["local_write"] = Value::String("allow".into());
    value["permissions"]["classes"]["shell_exec"] = Value::String("allow".into());
    value["permissions"]["classes"]["interactive_shell"] = Value::String("allow".into());
    value["permissions"]["classes"]["external"] = Value::String("allow".into());
    fs::write(config, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn prepend_stub_matchers(config: &Path, matchers: Vec<Value>) {
    let mut value: Value = serde_json::from_str(&fs::read_to_string(config).unwrap()).unwrap();
    let existing = value["stub"]["matchers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut combined = matchers;
    combined.extend(existing);
    value["stub"]["matchers"] = Value::Array(combined);
    fs::write(config, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                while let Some(next) = chars.next() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(ch);
    }
    out.replace('\r', "")
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn chat_works_through_oo_alias_end_to_end() {
    let temp = TempDir::new("noodle-e2e-chat");
    let config = write_test_config(&temp, "auto");
    let output = run_zsh(&temp, &config, "oo hello");
    assert!(
        output.contains("chat-ok"),
        "expected chat response, got:\n{}",
        output
    );
}

#[test]
fn installer_bootstraps_from_archive_when_piped_over_curl() {
    let temp = TempDir::new("noodle-installer-bootstrap");
    let home = temp.path().join("home");
    let install_root = temp.path().join("install-root");
    let fake_bin = temp.path().join("fake-bin");
    let archive_root = temp.path().join("archive-src");
    let repo_root = archive_root.join("noodle-main");
    let archive_path = temp.path().join("noodle-main.tar.gz");

    fs::create_dir_all(home.join("Library/LaunchAgents")).unwrap();
    fs::create_dir_all(&install_root).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(repo_root.join("scripts")).unwrap();
    fs::create_dir_all(repo_root.join("plugin")).unwrap();
    fs::create_dir_all(repo_root.join("config")).unwrap();
    fs::create_dir_all(repo_root.join("modules")).unwrap();
    fs::write(repo_root.join("Cargo.toml"), "[package]\nname = \"noodle\"\n").unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install.sh"),
        repo_root.join("scripts/install.sh"),
    )
    .unwrap();
    fs::write(
        repo_root.join("plugin/noodle.plugin.zsh"),
        "echo noodle-plugin-placeholder\n",
    )
    .unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("config/config.example.json"),
        repo_root.join("config/config.example.json"),
    )
    .unwrap();
    fs::write(repo_root.join("modules/.keep"), "").unwrap();

    write_executable(
        &fake_bin.join("cargo"),
        "#!/bin/sh\nset -eu\nif [ \"$1\" = \"build\" ] && [ \"$2\" = \"--release\" ]; then\n  mkdir -p target/release\n  printf '#!/bin/sh\\nexit 0\\n' > target/release/noodle\n  chmod +x target/release/noodle\n  exit 0\nfi\nprintf 'unexpected cargo args: %s\\n' \"$*\" >&2\nexit 1\n",
    );
    write_executable(&fake_bin.join("codesign"), "#!/bin/sh\nexit 0\n");
    write_executable(&fake_bin.join("launchctl"), "#!/bin/sh\nexit 0\n");

    let tar_status = Command::new("tar")
        .arg("-czf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&archive_root)
        .arg("noodle-main")
        .status()
        .unwrap();
    assert!(tar_status.success(), "failed to build installer archive");

    let path_env = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );
    let install_script_url = format!("file://{}", repo_root.join("scripts/install.sh").display());
    let archive_url = format!("file://{}", archive_path.display());
    let output = Command::new("/bin/zsh")
        .arg("-lc")
        .arg("curl -fsSL \"$INSTALL_SCRIPT_URL\" | zsh")
        .env("HOME", &home)
        .env("PATH", path_env)
        .env("INSTALL_SCRIPT_URL", install_script_url)
        .env("NOODLE_INSTALL_ARCHIVE_URL", archive_url)
        .env("NOODLE_INSTALL_ROOT", &install_root)
        .env("NOODLE_INSTALL_CONFIGURE_LLM", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "installer failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Installed noodle."),
        "expected installer success output, got:\n{}",
        combined
    );
    assert!(install_root.join("bin/noodle").exists());
    assert!(install_root.join("plugin/noodle.plugin.zsh").exists());
    assert!(install_root.join("config/config.example.json").exists());
    assert!(install_root.join("modules").exists());
    assert!(home.join("Library/LaunchAgents/com.noodle.daemon.plist").exists());

    let config: Value =
        serde_json::from_str(&fs::read_to_string(install_root.join("config.json")).unwrap())
            .unwrap();
    assert_eq!(config["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(config["reasoning_effort"].as_str(), Some("medium"));
    assert_eq!(config["timeout_seconds"].as_i64(), Some(30));
    assert_eq!(config["max_tokens"].as_i64(), Some(1024));
    let chat_tools = config["plugins"]["chat"]["uses_tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        chat_tools
            .iter()
            .any(|item| item.as_str() == Some("mcp_tools_list")),
        "{config}"
    );
    assert!(
        chat_tools
            .iter()
            .any(|item| item.as_str() == Some("mcp_tool_call")),
        "{config}"
    );
    assert!(
        chat_tools
            .iter()
            .any(|item| item.as_str() == Some("mcp_resources_list")),
        "{config}"
    );
}

#[test]
fn oo_dispatch_does_not_fetch_runtime_config() {
    let temp = TempDir::new("noodle-e2e-chat-thin-dispatch");
    let config = write_test_config(&temp, "auto");
    let marker = temp.path().join("runtime-config.log");
    let helper = temp.path().join("noodle-helper.sh");
    fs::write(
        &helper,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"runtime-config\" ]; then printf 'runtime-config\\n' >> '{}'\nfi\nexec '{}' \"$@\"\n",
            marker.display(),
            noodle_bin()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&helper).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&helper, perms).unwrap();
    }

    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");
    let output = Command::new("/bin/zsh")
        .arg("-ic")
        .arg(format!(
            "source '{}'; _noodle_chat_oo hello",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugin/noodle.plugin.zsh")
                .display(),
        ))
        .env("HOME", temp.path())
        .env("NOODLE_HELPER", &helper)
        .env("NOODLE_CONFIG", &config)
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1")
        .output()
        .unwrap();
    kill_daemon(&pidfile, &socket);
    let combined = strip_ansi(&format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
    assert!(
        combined.contains("chat-ok"),
        "expected chat response, got:\n{}",
        combined
    );
    assert!(
        fs::read_to_string(&marker)
            .unwrap_or_default()
            .trim()
            .is_empty(),
        "oo dispatch should not call runtime-config, got:\n{}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
}

#[test]
fn chat_works_through_prefix_alias_end_to_end() {
    let temp = TempDir::new("noodle-e2e-prefix");
    let config = write_test_config(&temp, "auto");
    let output = run_zsh(&temp, &config, ", test");
    assert!(
        output.contains("prefix-ok"),
        "expected prefix chat response, got:\n{}",
        output
    );
}

#[test]
fn chat_is_workspace_aware_end_to_end() {
    let temp = TempDir::new("noodle-e2e-chat-workspace-aware");
    let config = write_test_config(&temp, "auto");
    let output = run_zsh(&temp, &config, "oo inspect workspace context");
    assert!(
        output.contains("workspace-aware-ok"),
        "expected workspace-aware chat response, got:\n{}",
        output
    );
}

#[test]
fn todo_slash_commands_work_through_daemon() {
    let temp = TempDir::new("noodle-e2e-todo");
    let config = write_test_config(&temp, "auto");

    let added = run_mode(
        &temp,
        &config,
        "slash_command",
        "/todo add ship noodle",
        None,
    );
    assert_eq!(added["action"].as_str(), Some("message"));
    assert!(
        added["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Added todo #1: ship noodle")
    );

    let help = run_mode(&temp, &config, "slash_command", "/todo help", None);
    assert!(
        help["message"]
            .as_str()
            .unwrap_or_default()
            .contains("/todo / <id>")
    );

    let listed = run_mode(&temp, &config, "slash_command", "/todo list", None);
    let message = listed["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("#1 [ ] ship noodle"),
        "unexpected list:\n{}",
        listed
    );

    let partial = run_mode(&temp, &config, "slash_command", "/todo / 1", None);
    assert!(
        partial["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Marked todo #1 as partial")
    );

    let listed_partial = run_mode(&temp, &config, "slash_command", "/todo list", None);
    let partial_message = listed_partial["message"].as_str().unwrap_or_default();
    assert!(
        partial_message.contains("#1 [/] ship noodle"),
        "unexpected partial list:\n{}",
        listed_partial
    );

    let done = run_mode(&temp, &config, "slash_command", "/todo x 1", None);
    assert!(
        done["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Completed todo #1")
    );

    let cleared = run_mode(&temp, &config, "slash_command", "/todo clear-done", None);
    assert!(
        cleared["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Cleared 1 completed todo")
    );
}

#[test]
fn slash_command_runtime_config_and_adapter_path_are_available() {
    let temp = TempDir::new("noodle-e2e-slash-runtime");
    let config = write_test_config(&temp, "auto");
    let runtime = Command::new(noodle_bin())
        .arg("runtime-config")
        .arg("--config")
        .arg(&config)
        .env("HOME", temp.path())
        .env("NOODLE_BYPASS_DAEMON", "1")
        .output()
        .unwrap();
    let runtime_text = String::from_utf8_lossy(&runtime.stdout);
    assert!(runtime_text.contains("slash_commands=help status reload config memory kv todo"));

    let output = run_zsh(
        &temp,
        &config,
        "_noodle_dispatch_explicit_input '/todo add write docs'; _noodle_dispatch_explicit_input '/todo list'",
    );
    assert!(output.contains("Added todo #1: write docs"), "{}", output);
    assert!(output.contains("#1 [ ] write docs"), "{}", output);
}

#[test]
fn utils_slash_commands_work_through_daemon() {
    let temp = TempDir::new("noodle-e2e-utils");
    let config = write_test_config(&temp, "auto");

    let help = run_mode(&temp, &config, "slash_command", "/help", None);
    let help_text = help["message"].as_str().unwrap_or_default();
    assert!(
        help_text.contains("/help - Show available slash commands"),
        "{help}"
    );
    assert!(
        help_text.contains("/reload - Reload noodle runtime config in the current shell."),
        "{help}"
    );
    assert!(
        help_text.contains("/memory - Inspect, search, or clear noodle memory state."),
        "{help}"
    );
    assert!(
        help_text.contains("/kv - Shared key/value cache for scripting with optional TTL."),
        "{help}"
    );

    let status = run_mode(&temp, &config, "slash_command", "/status", None);
    let status_text = status["message"].as_str().unwrap_or_default();
    assert!(status_text.contains("Noodle status"), "{status}");
    assert!(status_text.contains("Plugins:"), "{status}");
    assert!(
        status_text.contains("Slash commands: /help /status /reload /config /memory /kv /todo"),
        "{status}"
    );
}

#[test]
fn reload_slash_command_refreshes_runtime_config_in_current_shell() {
    let temp = TempDir::new("noodle-e2e-reload");
    let config = write_test_config(&temp, "auto");

    let output = run_zsh(
        &temp,
        &config,
        r#"
_noodle_load_runtime_config
print -r -- "before:$NOODLE_AUTO_RUN"
python3 -c 'import json, sys; path = sys.argv[1]; data = json.load(open(path)); data["runtime"]["auto_run"] = 0; open(path, "w").write(json.dumps(data, indent=2) + "\n")' "$NOODLE_CONFIG"
_noodle_dispatch_explicit_input "/reload"
print -r -- "after:$NOODLE_AUTO_RUN"
"#,
    );
    assert!(output.contains("before:1"), "{}", output);
    assert!(
        output.contains("Reloaded noodle runtime config."),
        "{}",
        output
    );
    assert!(output.contains("after:0"), "{}", output);
}

#[test]
fn memory_and_config_slash_commands_work_through_daemon() {
    let temp = TempDir::new("noodle-e2e-memory-config");
    let config = write_test_config(&temp, "auto");

    let added = run_mode(
        &temp,
        &config,
        "slash_command",
        "/todo add search target",
        None,
    );
    assert!(
        added["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Added todo #1: search target")
    );

    let memory_summary = run_mode(&temp, &config, "slash_command", "/memory", None);
    let memory_text = memory_summary["message"].as_str().unwrap_or_default();
    assert!(memory_text.contains("Memory DB:"), "{memory_summary}");
    assert!(memory_text.contains("Plugins:"), "{memory_summary}");

    let memory_search = run_mode(
        &temp,
        &config,
        "slash_command",
        "/memory search search target",
        None,
    );
    let search_text = memory_search["message"].as_str().unwrap_or_default();
    assert!(
        search_text.contains("Memory search: search target"),
        "{memory_search}"
    );
    assert!(search_text.contains("search target"), "{memory_search}");

    let cache_set = run_mode(
        &temp,
        &config,
        "slash_command",
        "/kv set session-token abc123 --ttl 1s",
        None,
    );
    assert!(
        cache_set["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Set kv key session-token. TTL 1s"),
        "{cache_set}"
    );

    let cache_get = run_mode(
        &temp,
        &config,
        "slash_command",
        "/kv get session-token",
        None,
    );
    assert_eq!(cache_get["message"].as_str(), Some("abc123"));

    thread::sleep(Duration::from_millis(1100));

    let cache_expired = run_mode(
        &temp,
        &config,
        "slash_command",
        "/kv get session-token",
        None,
    );
    assert!(
        cache_expired["message"]
            .as_str()
            .unwrap_or_default()
            .contains("KV key not found: session-token"),
        "{cache_expired}"
    );

    let cache_set_persistent = run_mode(
        &temp,
        &config,
        "slash_command",
        "/kv set shell-cache keep-me",
        None,
    );
    assert!(
        cache_set_persistent["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Set kv key shell-cache."),
        "{cache_set_persistent}"
    );

    let cache_unset = run_mode(
        &temp,
        &config,
        "slash_command",
        "/kv unset shell-cache",
        None,
    );
    assert!(
        cache_unset["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Removed kv key shell-cache."),
        "{cache_unset}"
    );

    let config_get = run_mode(
        &temp,
        &config,
        "slash_command",
        "/config get plugins.chat.prefix",
        None,
    );
    assert_eq!(config_get["message"].as_str(), Some(","));

    let config_set = run_mode(
        &temp,
        &config,
        "slash_command",
        "/config set runtime.debug true",
        None,
    );
    assert!(
        config_set["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Updated runtime.debug"),
        "{config_set}"
    );

    let config_get_debug = run_mode(
        &temp,
        &config,
        "slash_command",
        "/config get runtime.debug",
        None,
    );
    assert_eq!(config_get_debug["message"].as_str(), Some("true"));

    let memory_clear = run_mode(&temp, &config, "slash_command", "/memory clear todo", None);
    assert!(
        memory_clear["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Cleared memory for todo"),
        "{memory_clear}"
    );

    let empty_todos = run_mode(&temp, &config, "slash_command", "/todo list", None);
    assert!(
        empty_todos["message"]
            .as_str()
            .unwrap_or_default()
            .contains("No todos yet."),
        "{empty_todos}"
    );
}

#[test]
fn typos_autoruns_first_selection_end_to_end() {
    let temp = TempDir::new("noodle-e2e-typos");
    let config = write_test_config(&temp, "auto");
    let output = run_zsh(&temp, &config, "die");
    assert!(
        output.contains("typo-ok"),
        "expected typo correction to execute, got:\n{}",
        output
    );
}

#[test]
fn typo_selected_updates_memory_through_external_module() {
    let temp = TempDir::new("noodle-e2e-typo-selected");
    let config = write_test_config(&temp, "select");
    let payload = run_mode(&temp, &config, "typo_selected", "die", Some("echo typo-ok"));
    assert_eq!(payload["action"], "noop");
    assert_eq!(payload["plugin"], "typos");

    let memory = run_mode(
        &temp,
        &config,
        "slash_command",
        "/memory search typo-ok",
        None,
    );
    let message = memory["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("typos.selection"),
        "expected typo selection event in memory search, got:\n{}",
        message
    );
    assert!(
        message.contains("echo typo-ok"),
        "expected selected command in memory search, got:\n{}",
        message
    );
}

#[test]
fn chat_can_chain_a_tool_call_end_to_end() {
    let temp = TempDir::new("noodle-e2e-tool-chain");
    let config = write_test_config(&temp, "auto");
    let output = run_zsh(&temp, &config, "oo read the chained file");
    assert!(
        output.contains("tool-chain-ok"),
        "expected chained tool response, got:\n{}",
        output
    );
}

#[test]
fn chat_resolves_relative_file_reads_from_request_cwd() {
    let temp = TempDir::new("noodle-e2e-relative-readme");
    let config = write_test_config(&temp, "auto");
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo what does readme.md say?",
        None,
    );
    assert_eq!(result["action"], "batch");
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("relative-readme-ok")
    );
}

#[test]
fn chat_rechecks_a_bad_direct_answer_with_tools() {
    let temp = TempDir::new("noodle-e2e-grounding-check");
    let config = write_test_config(&temp, "auto");
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo find the readme file in claude-code-main",
        None,
    );
    let expected = temp
        .path()
        .join("claude-code-main")
        .join("README.md")
        .display()
        .to_string();
    assert_eq!(result["action"], "batch");
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some(expected.as_str())
    );
}

#[test]
fn chat_recovers_from_meta_tool_reply_for_guidance_request() {
    let temp = TempDir::new("noodle-e2e-guidance-recovery");
    let config = write_test_config(&temp, "auto");
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "[Task Directive]\nYou must produce a useful answer now.\nReason: The user asked for guidance or an example command/script, not for you to inspect or execute anything on their behalf.",
                "response": "FINAL: Use:\nfind / -type f -iname 'README.md' 2>/dev/null\nPut that in a script like find-readmes.sh and run chmod +x find-readmes.sh."
            }),
            json!({
                "contains": "[Task Directive]\nYou previously drafted a direct answer before using any tools.\nDraft answer:\nI need to inspect with tools before I can answer that confidently.",
                "response": "FINAL: I still need to inspect my tools."
            }),
            json!({
                "contains": "[Current Request]\nhow do i write a bash script to find every readme.md file on my entire computer?",
                "response": "FINAL: I need to inspect with tools before I can answer that confidently."
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo how do i write a bash script to find every readme.md file on my entire computer?",
        None,
    );
    assert_eq!(result["action"], "message");
    let message = result["message"].as_str().unwrap_or("");
    assert!(message.contains("find / -type f -iname 'README.md' 2>/dev/null"));
    assert!(!message.contains("inspect with tools"));
}

#[test]
fn chat_refuses_recursive_noodle_invocation_and_recovers() {
    let temp = TempDir::new("noodle-e2e-recursive-shell-guard");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "TOOL_RESULT: shell_exec {\"command\":\"printf recovered-direct-command\"",
                "response": "FINAL: recovered-direct-command"
            }),
            json!({
                "contains": "Refused recursive noodle invocation.",
                "response": "TOOL: shell_exec {\"command\":\"printf recovered-direct-command\"}"
            }),
            json!({
                "contains": "[Current Request]\nstart claude interactive mode and ask it to create a CLAUDE.md for this project and then exit",
                "response": "TOOL: shell_exec {\"command\":\"bash -lc 'oo start claude interactive mode and ask it to create a CLAUDE.md for this project and then exit'\"}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo start claude interactive mode and ask it to create a CLAUDE.md for this project and then exit",
        None,
    );
    assert_eq!(result["action"], "batch");
    let items = result["items"].as_array().unwrap();
    assert!(items.iter().any(|item| {
        item["action"].as_str() == Some("tool_step")
            && item["summary"]
                .as_str()
                .unwrap_or("")
                .contains("Refused recursive noodle invocation")
    }));
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("recovered-direct-command")
    );
}

#[test]
fn interactive_shell_read_waits_for_output_to_settle() {
    let temp = TempDir::new("noodle-e2e-interactive-settle");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);

    let batch = run_tool_batch(
        &temp,
        &config,
        json!([
            {
                "tool": "interactive_shell_start",
                "args": {
                    "command": "sleep 0.15; printf first; sleep 0.15; printf second"
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "wait_ms": 800,
                    "settle_ms": 250
                }
            },
            {
                "tool": "interactive_shell_close",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id"
                }
            }
        ]),
    );
    let results = batch["results"].as_array().unwrap();
    let output = results[1]["output"]["output"].as_str().unwrap_or("");
    assert!(output.contains("first"));
    assert!(output.contains("second"));
}

#[test]
fn chat_keeps_reading_when_interactive_session_is_still_open() {
    let temp = TempDir::new("noodle-e2e-interactive-autoread");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "TOOL_RESULT: interactive_shell_read",
                "response": "FINAL: delayed-done"
            }),
            json!({
                "contains": "TOOL_RESULT: interactive_shell_start",
                "response": "FINAL:"
            }),
            json!({
                "contains": "[Current Request]\nwait for delayed interactive output",
                "response": "TOOL: interactive_shell_start {\"command\":\"sleep 0.2; printf delayed-done\"}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo wait for delayed interactive output",
        None,
    );
    assert_eq!(result["action"], "batch");
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("delayed-done")
    );
}

#[test]
fn chat_waits_through_multiple_idle_polls_for_interactive_output() {
    let temp = TempDir::new("noodle-e2e-interactive-multi-idle");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "TOOL_RESULT: interactive_shell_read",
                "response": "FINAL: delayed-done"
            }),
            json!({
                "contains": "TOOL_RESULT: interactive_shell_start",
                "response": "FINAL:"
            }),
            json!({
                "contains": "[Current Request]\nwait through a slow interactive startup",
                "response": "TOOL: interactive_shell_start {\"command\":\"sleep 4.5; printf delayed-done\"}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo wait through a slow interactive startup",
        None,
    );
    assert_eq!(result["action"], "batch");
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("delayed-done")
    );
}

#[test]
fn chat_waits_for_output_after_interactive_write() {
    let temp = TempDir::new("noodle-e2e-interactive-write-wait");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "Task finished. Respond with FINAL only.",
                "response": "FINAL: done:alex"
            }),
            json!({
                "contains": "[Current Request]\nwait for output after writing to an interactive session",
                "response": "PLAN: drive interactive session through a write\nSTEP: interactive_shell_start {\"command\":\"sleep 0.1; printf prompt:; read line; sleep 0.6; printf done:$line\"}\nSTEP: interactive_shell_write {\"session_id\":\"__TOOL_RESULT_0__.output.session_id\",\"text\":\"alex\",\"submit\":true}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo wait for output after writing to an interactive session",
        None,
    );
    assert_eq!(result["action"], "batch");
    let items = result["items"].as_array().unwrap();
    assert!(items.iter().any(|item| {
        item["action"].as_str() == Some("session_output")
            && item["text"].as_str().unwrap_or("").contains("done:alex")
    }));
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("done:alex"),
        "unexpected interactive write batch result:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );
}

#[test]
fn chat_can_handle_a_numbered_interactive_approval_menu() {
    let temp = TempDir::new("noodle-e2e-interactive-approval-menu");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "selected:1",
                "response": "FINAL: selected 1"
            }),
            json!({
                "contains": "An interactive shell session is still open",
                "response": "TOOL: interactive_shell_key {\"session_id\":\"__TOOL_RESULT_0__.output.session_id\",\"key\":\"Enter\"}"
            }),
            json!({
                "contains": "[Current Request]\nhandle an interactive approval menu",
                "response": "PLAN: choose the temporary approval option\nSTEP: interactive_shell_start {\"command\":\"printf \\\"Do you want to proceed?\\\\n❯ 1. Yes\\\\n  2. Yes, and don't ask again\\\\n  3. No\\\\n\\\"; old=$(stty -g); stty raw -echo; b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\\\n'); stty \\\"$old\\\"; if [ \\\"$b\\\" = \\\"0d\\\" ]; then printf 'selected:%s' 1; else printf 'selected:%s' \\\"$b\\\"; fi\"}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo handle an interactive approval menu",
        None,
    );
    assert_eq!(result["action"], "batch");
    let items = result["items"].as_array().unwrap();
    assert!(items.iter().any(|item| {
        item["action"].as_str() == Some("session_output")
            && item["text"]
                .as_str()
                .unwrap_or("")
                .contains("Do you want to proceed?")
    }));
    assert!(
        items.iter().any(|item| {
            item["action"].as_str() == Some("session_input")
                && item["text"].as_str().unwrap_or("").contains("<Enter>")
        }),
        "expected session_input approval choice, got:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("selected 1")
    );
}

#[test]
fn chat_can_handle_a_long_screen_interactive_approval_menu() {
    let temp = TempDir::new("noodle-e2e-interactive-long-approval-menu");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "selected:1",
                "response": "FINAL: selected 1"
            }),
            json!({
                "contains": "An interactive shell session is still open",
                "response": "TOOL: interactive_shell_key {\"session_id\":\"__TOOL_RESULT_0__.output.session_id\",\"key\":\"Enter\"}"
            }),
            json!({
                "contains": "[Current Request]\nhandle a long interactive approval menu",
                "response": "PLAN: choose the temporary approval option after a long screen\nSTEP: interactive_shell_start {\"command\":\"i=0; while [ $i -lt 700 ]; do printf 'banner line\\\\n'; i=$((i+1)); done; printf \\\"Do you want to proceed?\\\\n❯ 1. Yes\\\\n  2. Yes, and don't ask again\\\\n  3. No\\\\n\\\"; old=$(stty -g); stty raw -echo; b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\\\n'); stty \\\"$old\\\"; if [ \\\"$b\\\" = \\\"0d\\\" ]; then printf 'selected:%s' 1; else printf 'selected:%s' \\\"$b\\\"; fi\"}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo handle a long interactive approval menu",
        None,
    );
    assert_eq!(result["action"], "batch");
    let items = result["items"].as_array().unwrap();
    assert!(
        items.iter().any(|item| {
            item["action"].as_str() == Some("session_input")
                && item["text"].as_str().unwrap_or("").contains("<Enter>")
        }),
        "expected session_input approval choice, got:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("selected 1")
    );
}

#[test]
fn chat_can_handle_a_highlighted_interactive_approval_menu() {
    let temp = TempDir::new("noodle-e2e-interactive-highlighted-approval-menu");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "selected:1",
                "response": "FINAL: selected 1"
            }),
            json!({
                "contains": "An interactive shell session is still open",
                "response": "TOOL: interactive_shell_key {\"session_id\":\"__TOOL_RESULT_0__.output.session_id\",\"key\":\"Enter\"}"
            }),
            json!({
                "contains": "[Current Request]\nhandle a highlighted interactive approval menu",
                "response": "PLAN: choose the highlighted temporary approval option\nSTEP: interactive_shell_start {\"command\":\"printf \\\"Do you want to proceed?\\\\n❯ 1. Yes\\\\n  2. Yes, and don't ask again\\\\n  3. No\\\\n\\\"; old=$(stty -g); stty raw -echo; b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\\\n'); stty \\\"$old\\\"; if [ \\\"$b\\\" = \\\"0d\\\" ]; then printf 'selected:%s' 1; else printf 'selected:%s' \\\"$b\\\"; fi\"}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo handle a highlighted interactive approval menu",
        None,
    );
    assert_eq!(result["action"], "batch");
    let items = result["items"].as_array().unwrap();
    assert!(
        items.iter().any(|item| {
            item["action"].as_str() == Some("session_input")
                && item["text"].as_str().unwrap_or("").contains("<Enter>")
        }),
        "expected highlighted menu approval choice, got:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("selected 1")
    );
}

#[test]
fn planned_task_keeps_driving_an_interactive_menu_before_finalizing() {
    let temp = TempDir::new("noodle-e2e-interactive-plan-finalize-menu");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "selected:1",
                "response": "FINAL: selected 1"
            }),
            json!({
                "contains": "An interactive shell session is still open",
                "response": "TOOL: interactive_shell_key {\"session_id\":\"__TOOL_RESULT_0__.output.session_id\",\"key\":\"Enter\"}"
            }),
            json!({
                "contains": "Task finished. Respond with FINAL only.",
                "response": "FINAL: I’ve started Claude’s interactive CLI and it may still be awaiting approval."
            }),
            json!({
                "contains": "[Current Request]\nfinish the interactive menu task",
                "response": "PLAN: drive the menu to completion\nSTEP: interactive_shell_start {\"command\":\"printf \\\"Do you want to proceed?\\\\n❯ 1. Yes\\\\n  2. Yes, and don't ask again\\\\n  3. No\\\\n\\\"; old=$(stty -g); stty raw -echo; b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\\\n'); stty \\\"$old\\\"; if [ \\\"$b\\\" = \\\"0d\\\" ]; then printf 'selected:%s' 1; else printf 'selected:%s' \\\"$b\\\"; fi\"}"
            }),
        ],
    );
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo finish the interactive menu task",
        None,
    );
    assert_eq!(result["action"], "batch");
    let items = result["items"].as_array().unwrap();
    assert!(
        items.iter().any(|item| {
            item["action"].as_str() == Some("session_input")
                && item["text"].as_str().unwrap_or("").contains("<Enter>")
        }),
        "expected session_input approval choice, got:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("selected 1")
    );
}

#[test]
fn interactive_shell_read_preserves_display_ansi_but_strips_control_noise() {
    let temp = TempDir::new("noodle-e2e-interactive-ansi");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);

    let batch = run_tool_batch(
        &temp,
        &config,
        json!([
            {
                "tool": "interactive_shell_start",
                "args": {
                    "command": "printf '\\033[31mred\\033[0m\\033[?1;2c'"
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "wait_ms": 800,
                    "settle_ms": 250
                }
            },
            {
                "tool": "interactive_shell_close",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id"
                }
            }
        ]),
    );
    let results = batch["results"].as_array().unwrap();
    let plain = results[1]["output"]["output"].as_str().unwrap_or("");
    let display = results[1]["output"]["display_output"]
        .as_str()
        .unwrap_or("");
    assert!(plain.contains("red"));
    assert!(!plain.contains("\u{1b}[?1;2c"));
    assert!(display.contains("red"));
    assert!(display.contains('\u{1b}'));
    assert!(!display.contains("\u{1b}[?1;2c"));
    assert!(!display.contains("\u{1b}[H"));
    assert!(!display.contains("\u{1b}[J"));
    assert!(!display.contains("\u{1b}[?25h"));
}

#[test]
fn chat_can_still_summarize_when_explicitly_requested() {
    let temp = TempDir::new("noodle-e2e-summary-request");
    let config = write_test_config(&temp, "auto");
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo summarize readme.md briefly",
        None,
    );
    assert_eq!(result["action"], "batch");
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("concise-readme-summary")
    );
}

#[test]
fn chat_can_execute_a_multi_step_plan_end_to_end() {
    let temp = TempDir::new("noodle-e2e-task-plan");
    let config = write_test_config(&temp, "auto");
    let output = run_zsh(&temp, &config, "oo inspect the workspace deeply");
    assert!(
        output.contains("I inspected both files and summarized them."),
        "expected planned task response, got:\n{}",
        output
    );

    let memory_query = run_tool(
        &temp,
        &config,
        "memory_query",
        json!({"plugin": "tasks", "source": "artifacts", "limit": 10}),
    );
    let artifacts = memory_query["output"]["artifacts"].as_array().unwrap();
    assert!(
        artifacts.iter().any(|item| {
            item["content"]
                .as_str()
                .unwrap_or("")
                .contains("\"status\": \"completed\"")
        }),
        "expected completed task artifact, got:\n{}",
        memory_query
    );
}

#[test]
fn chat_can_replan_after_a_failed_task_step() {
    let temp = TempDir::new("noodle-e2e-task-replan");
    let config = write_test_config(&temp, "auto");
    let output = run_zsh(&temp, &config, "oo recover from a broken plan");
    assert!(
        output.contains("I recovered after the failed step."),
        "expected replanned task response, got:\n{}",
        output
    );

    let memory_query = run_tool(
        &temp,
        &config,
        "memory_query",
        json!({"plugin": "tasks", "source": "artifacts", "limit": 10}),
    );
    let artifacts = memory_query["output"]["artifacts"].as_array().unwrap();
    assert!(
        artifacts.iter().any(|item| {
            let content = item["content"].as_str().unwrap_or("");
            content.contains("\"status\": \"completed\"")
                && content.contains("fallback after failure")
        }),
        "expected replanned completed task artifact, got:\n{}",
        memory_query
    );
}

#[test]
fn planned_task_returns_progress_batch() {
    let temp = TempDir::new("noodle-e2e-task-batch");
    let config = write_test_config(&temp, "auto");
    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo inspect the workspace deeply",
        None,
    );
    assert_eq!(result["action"].as_str(), Some("batch"));
    let items = batch_items(&result);
    assert!(
        items
            .iter()
            .any(|item| item["action"].as_str() == Some("task_started"))
    );
    assert!(
        items
            .iter()
            .any(|item| item["action"].as_str() == Some("task_step"))
    );
    assert!(
        items
            .iter()
            .any(|item| item["action"].as_str() == Some("task_finished"))
    );
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["action"].as_str()),
        Some("message")
    );
}

#[test]
fn task_commands_list_show_resume_and_cancel() {
    let temp = TempDir::new("noodle-e2e-task-commands");
    let config = write_test_config(&temp, "auto");

    let paused = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo write the planned file",
        None,
    );
    let paused_items = batch_items(&paused);
    let task_id = paused_items
        .iter()
        .find(|item| item["action"].as_str() == Some("task_started"))
        .and_then(|item| item["task_id"].as_str())
        .unwrap()
        .to_string();

    let list = run_task_command(
        &temp,
        &config,
        "task-list",
        &["--status", "awaiting_permission"],
    );
    let tasks = list["tasks"].as_array().unwrap();
    assert!(
        tasks
            .iter()
            .any(|task| task["id"].as_str() == Some(task_id.as_str()))
    );

    let show = run_task_command(&temp, &config, "task-show", &["--task-id", &task_id]);
    assert_eq!(show["task"]["id"].as_str(), Some(task_id.as_str()));
    assert_eq!(show["task"]["status"].as_str(), Some("awaiting_permission"));
    assert_eq!(
        show["runtime"]["task"]["id"].as_str(),
        Some(task_id.as_str())
    );

    let resumed = run_task_command(&temp, &config, "task-resume", &["--task-id", &task_id]);
    assert_eq!(resumed["action"].as_str(), Some("batch"));
    let resumed_last = last_batch_action(&resumed).unwrap();
    assert_eq!(resumed_last["action"].as_str(), Some("permission_request"));

    let cancelled = run_task_command(&temp, &config, "task-cancel", &["--task-id", &task_id]);
    assert_eq!(cancelled["task"]["status"].as_str(), Some("cancelled"));

    let show_cancelled = run_task_command(&temp, &config, "task-show", &["--task-id", &task_id]);
    assert_eq!(show_cancelled["task"]["status"].as_str(), Some("cancelled"));
    assert!(show_cancelled["runtime"].is_null());
}

#[test]
fn permission_request_can_resume_shell_execution() {
    let temp = TempDir::new("noodle-e2e-permission-shell");
    let config = write_test_config(&temp, "auto");
    let first = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo run the shell task",
        None,
    );
    assert_eq!(first["action"].as_str(), Some("permission_request"));
    assert_eq!(first["tool"].as_str(), Some("shell_exec"));
    let permission_id = first["permission_id"].as_str().unwrap().to_string();

    let resumed = run_mode(
        &temp,
        &config,
        "permission_response",
        &permission_id,
        Some("allow"),
    );
    assert_eq!(resumed["action"].as_str(), Some("batch"));
    assert_eq!(
        last_batch_action(&resumed).and_then(|item| item["message"].as_str()),
        Some("shell execution completed.")
    );
}

#[test]
fn permission_request_can_resume_planned_write() {
    let temp = TempDir::new("noodle-e2e-permission-plan");
    let config = write_test_config(&temp, "auto");
    let first = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo write the planned file",
        None,
    );
    assert_eq!(first["action"].as_str(), Some("batch"));
    let first_last = last_batch_action(&first).unwrap();
    assert_eq!(first_last["action"].as_str(), Some("permission_request"));
    assert_eq!(first_last["tool"].as_str(), Some("file_write"));
    let permission_id = first_last["permission_id"].as_str().unwrap().to_string();

    let resumed = run_mode(
        &temp,
        &config,
        "permission_response",
        &permission_id,
        Some("allow"),
    );
    assert_eq!(resumed["action"].as_str(), Some("batch"));
    assert_eq!(
        last_batch_action(&resumed).and_then(|item| item["message"].as_str()),
        Some("I wrote the file successfully.")
    );
    let written_path = temp.path().join("write-plan.txt");
    assert_eq!(fs::read_to_string(written_path).unwrap(), "written-by-plan");
}

#[test]
fn permission_request_reuses_same_mcp_tool_for_rest_of_task() {
    let temp = TempDir::new("noodle-e2e-permission-mcp-tool");
    let config = write_test_config(&temp, "auto");
    prepend_stub_matchers(
        &config,
        vec![
            json!({
                "contains": "content_text:\ncounter:2",
                "response": "FINAL: counter:2"
            }),
            json!({
                "contains": "content_text:\ncounter:1",
                "response": "TOOL: mcp_tool_call {\"server\":\"docs\",\"tool\":\"counter\",\"arguments\":{}}"
            }),
            json!({
                "contains": "[Current Request]\nuse the docs mcp counter twice",
                "response": "TOOL: mcp_tool_call {\"server\":\"docs\",\"tool\":\"counter\",\"arguments\":{}}"
            }),
        ],
    );

    let first = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo use the docs mcp counter twice",
        None,
    );
    assert_eq!(first["action"].as_str(), Some("permission_request"));
    assert_eq!(first["tool"].as_str(), Some("mcp_tool_call"));
    let permission_id = first["permission_id"].as_str().unwrap().to_string();

    let resumed = run_mode(
        &temp,
        &config,
        "permission_response",
        &permission_id,
        Some("allow"),
    );
    assert_eq!(resumed["action"].as_str(), Some("batch"));
    let items = resumed["items"].as_array().unwrap();
    assert!(
        !items
            .iter()
            .any(|item| item["action"].as_str() == Some("permission_request")),
        "expected mcp_tool_call approval to persist within the task, got:\n{}",
        serde_json::to_string_pretty(&resumed).unwrap()
    );
    assert_eq!(
        last_batch_action(&resumed).and_then(|item| item["message"].as_str()),
        Some("counter:2"),
        "expected second counter call to stay on the same MCP session, got:\n{}",
        serde_json::to_string_pretty(&resumed).unwrap()
    );
}

#[test]
fn permission_request_can_be_denied() {
    let temp = TempDir::new("noodle-e2e-permission-deny");
    let config = write_test_config(&temp, "auto");
    let first = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo run the shell task",
        None,
    );
    assert_eq!(first["action"].as_str(), Some("permission_request"));
    let permission_id = first["permission_id"].as_str().unwrap().to_string();

    let resumed = run_mode(
        &temp,
        &config,
        "permission_response",
        &permission_id,
        Some("deny"),
    );
    assert_eq!(resumed["action"].as_str(), Some("message"));
    assert!(
        resumed["message"]
            .as_str()
            .unwrap_or("")
            .contains("Permission denied")
    );
}

#[test]
fn chat_can_drive_an_interactive_shell_session() {
    let temp = TempDir::new("noodle-e2e-interactive-shell-chat");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);

    let result = run_mode(
        &temp,
        &config,
        "command_not_found",
        "oo talk to the interactive harness and tell it alex",
        None,
    );
    assert_eq!(result["action"].as_str(), Some("batch"));
    assert_eq!(
        last_batch_action(&result).and_then(|item| item["message"].as_str()),
        Some("interactive shell completed."),
        "unexpected interactive shell batch result:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );
}

#[test]
fn zsh_streams_interactive_shell_progress_live() {
    let temp = TempDir::new("noodle-zsh-stream-interactive");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);

    let output = run_zsh(
        &temp,
        &config,
        "oo talk to the interactive harness and tell it alex",
    );
    assert!(
        output.contains("$ printf 'name? '; read name; printf 'hi:%s' $name"),
        "expected transcript command launch, got:\n{}",
        output
    );
    assert!(
        output.contains("name?") || output.contains("hi:alex"),
        "expected streamed shell output, got:\n{}",
        output
    );
    assert!(
        !output.contains("STEP:") && !output.contains("FINAL:"),
        "expected protocol lines to stay out of terminal output, got:\n{}",
        output
    );
    assert!(
        output.contains("interactive shell completed."),
        "expected final completion message, got:\n{}",
        output
    );
}

#[test]
fn zsh_streams_simple_tool_steps_live() {
    let temp = TempDir::new("noodle-zsh-stream-tools");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);

    let output = run_zsh(&temp, &config, "oo what does readme.md say\\?");
    assert!(
        output.contains("Reading README.md"),
        "expected file_read running step, got:\n{}",
        output
    );
    assert!(
        output.contains("Read /") || output.contains("Read README.md"),
        "expected file_read done step, got:\n{}",
        output
    );
    assert!(
        output.contains("## What is noodle?")
            || output.contains("A local-first terminal companion for `zsh`."),
        "expected raw file content, got:\n{}",
        output
    );
}

#[test]
fn zsh_renders_multiline_messages_as_a_single_avatar_block() {
    let temp = TempDir::new("noodle-zsh-multiline-block");
    let config = write_test_config(&temp, "auto");
    fs::write(
        temp.path().join("README.md"),
        "#!/usr/bin/env bash\necho alpha\necho beta",
    )
    .unwrap();

    let script = format!(
        "cd '{}'; oo what does readme.md say\\?",
        temp.path().display()
    );
    let output = run_zsh(&temp, &config, &script);
    assert!(
        output.contains("oo\n#!/usr/bin/env bash\necho alpha\necho beta"),
        "expected multiline block rendering, got:\n{}",
        output
    );
    assert!(
        !output.contains("oo\techo alpha") && !output.contains("oo\techo beta"),
        "expected subsequent lines without avatar prefix, got:\n{}",
        output
    );
}

#[test]
fn zsh_renders_multiline_slash_command_messages_as_single_avatar_blocks() {
    let temp = TempDir::new("noodle-zsh-slash-multiline-blocks");
    let config = write_test_config(&temp, "auto");

    let output = run_zsh(
        &temp,
        &config,
        "_noodle_dispatch_explicit_input '/help'; _noodle_dispatch_explicit_input '/memory help'; _noodle_dispatch_explicit_input '/todo help'; _noodle_dispatch_explicit_input '/kv help'",
    );
    assert!(
        output.contains("oo\nSlash commands:\n/help - Show available slash commands"),
        "expected utils multiline block rendering, got:\n{}",
        output
    );
    assert!(
        output.contains("oo\nMemory commands:\n/memory\n/memory help"),
        "expected memory multiline block rendering, got:\n{}",
        output
    );
    assert!(
        output.contains("oo\nTodo commands:\n/todo list\n/todo add <task>"),
        "expected todo multiline block rendering, got:\n{}",
        output
    );
    assert!(
        output.contains("oo\nScripting commands:\n/kv help\n/kv get <key>"),
        "expected scripting multiline block rendering, got:\n{}",
        output
    );
    assert!(
        !output.contains("oo\t/memory")
            && !output.contains("oo\t/todo")
            && !output.contains("oo\t/kv")
            && !output.contains("oo\t/help"),
        "expected slash command bodies without repeated avatar prefixes, got:\n{}",
        output
    );
}

#[test]
fn zsh_restores_cursor_after_terminal_cleanup() {
    let temp = TempDir::new("noodle-zsh-terminal-restore");
    let config = write_test_config(&temp, "auto");

    let output = run_zsh_raw(
        &temp,
        &config,
        "NOODLE_RAW_SESSION_OUTPUT_ACTIVE=1; NOODLE_TERMINAL_NEEDS_RESET=1; _noodle_finish_raw_output_if_needed",
    );
    assert!(
        output.contains("\u{1b}[0m\u{1b}[?25h\n"),
        "expected terminal reset and cursor restore, got:\n{:?}",
        output
    );
}

#[test]
fn zsh_restores_cursor_when_stream_helper_is_interrupted() {
    let temp = TempDir::new("noodle-zsh-interrupt-restore");
    let config = write_test_config(&temp, "auto");
    let combined = run_zsh_raw(
        &temp,
        &config,
        "NOODLE_STATUS_LINE_ACTIVE=1; NOODLE_TERMINAL_NEEDS_RESET=1; kill -INT $$",
    );
    assert!(
        combined.contains("\u{1b}[?25h"),
        "expected cursor restore on interrupt, got:\n{:?}",
        combined
    );
}

#[test]
fn daemon_exposes_tool_registry_and_builtin_tool_calls() {
    let temp = TempDir::new("noodle-e2e-tools");
    let config = write_test_config(&temp, "auto");
    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");

    let tool_list = Command::new(noodle_bin())
        .arg("tool-list")
        .arg("--config")
        .arg(&config)
        .arg("--plugin")
        .arg("chat")
        .env("HOME", temp.path())
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1")
        .output()
        .unwrap();
    let tool_list_text = String::from_utf8_lossy(&tool_list.stdout);
    for tool_name in [
        "memory_query",
        "file_read",
        "path_search",
        "glob",
        "grep",
        "web_fetch",
        "web_search",
        "file_write",
        "file_edit",
        "shell_exec",
        "interactive_shell_start",
        "interactive_shell_read",
        "interactive_shell_write",
        "interactive_shell_key",
        "interactive_shell_close",
        "mcp_tools_list",
        "mcp_tool_call",
        "mcp_resources_list",
        "mcp_resource_read",
        "task_note_write",
        "agent_handoff_create",
    ] {
        assert!(
            tool_list_text.contains(&format!("\"{tool_name}\"")),
            "expected tool registry output to include {}, got:\n{}",
            tool_name,
            tool_list_text
        );
    }

    let sample_file = temp.path().join("sample.txt");
    fs::write(&sample_file, "hello from tool").unwrap();
    let tool_call = run_tool(
        &temp,
        &config,
        "file_read",
        json!({"path": sample_file.display().to_string()}),
    );
    kill_daemon(&pidfile, &socket);
    assert!(
        tool_call["output"]["content"].as_str() == Some("hello from tool"),
        "expected file_read tool result, got:\n{}",
        tool_call
    );
}

#[test]
fn module_api_info_reports_versioned_host_contract() {
    let temp = TempDir::new("noodle-e2e-module-api-info");
    let socket = temp.path().join("noodle.sock");
    let pidfile = temp.path().join("noodle.pid");
    let output = Command::new(noodle_bin())
        .arg("module-api")
        .arg("info")
        .env("HOME", temp.path())
        .env("NOODLE_SOCKET", &socket)
        .env("NOODLE_PIDFILE", &pidfile)
        .env("NOODLE_BYPASS_DAEMON", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "module-api info failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["version"].as_str(), Some("v1"));
    assert_eq!(payload["command_prefix"][1].as_str(), Some("module-api"));
    let capabilities = payload["capabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        capabilities
            .iter()
            .any(|item| item.as_str() == Some("execution_run")),
        "{payload}"
    );
    assert!(
        capabilities
            .iter()
            .any(|item| item.as_str() == Some("tool_batch")),
        "{payload}"
    );
}

#[test]
fn client_fails_cleanly_when_daemon_is_not_running() {
    let temp = TempDir::new("noodle-daemon-down");
    let config = write_test_config(&temp, "select");
    let output = run_client_failure(
        &temp,
        &config,
        &["runtime-config", "--config", &config.display().to_string()],
    );
    assert!(output.contains("noodle daemon is not running or unreachable"));
    assert!(output.contains("launchctl kickstart -k gui/$(id -u)/com.noodle.daemon"));
}

#[test]
fn all_builtin_primitives_are_covered_and_work() {
    let temp = TempDir::new("noodle-e2e-all-tools");
    let config = write_test_config(&temp, "auto");

    let root = temp.path().join("workspace");
    fs::create_dir_all(root.join("nested")).unwrap();
    let alpha = root.join("alpha.txt");
    let beta = root.join("nested/beta.txt");
    fs::write(&alpha, "alpha\nneedle here\n").unwrap();
    fs::write(&beta, "beta line\n").unwrap();

    let file_read = run_tool(
        &temp,
        &config,
        "file_read",
        json!({"path": alpha.display().to_string()}),
    );
    assert_eq!(
        file_read["output"]["content"].as_str(),
        Some("alpha\nneedle here\n")
    );

    let village_dir = root.join("Tobacco Branch Village POA");
    fs::create_dir_all(&village_dir).unwrap();
    let agenda = village_dir.join("agenda.txt");
    fs::write(&agenda, "meeting notes").unwrap();

    let path_search_dir = run_tool(
        &temp,
        &config,
        "path_search",
        json!({
            "root": root.display().to_string(),
            "query": "tobacco branch village",
            "kind": "dir",
            "limit": 10
        }),
    );
    let path_search_dir_matches = path_search_dir["output"]["matches"].as_array().unwrap();
    assert!(
        path_search_dir_matches
            .iter()
            .any(|item| item.as_str() == Some(village_dir.to_string_lossy().as_ref()))
    );

    let path_search_file = run_tool(
        &temp,
        &config,
        "path_search",
        json!({
            "root": root.display().to_string(),
            "query": "agenda",
            "kind": "file",
            "limit": 10
        }),
    );
    let path_search_file_matches = path_search_file["output"]["matches"].as_array().unwrap();
    assert!(
        path_search_file_matches
            .iter()
            .any(|item| item.as_str() == Some(agenda.to_string_lossy().as_ref()))
    );

    let glob = run_tool(
        &temp,
        &config,
        "glob",
        json!({"root": root.display().to_string(), "pattern": "*.txt", "limit": 10}),
    );
    let glob_matches = glob["output"]["matches"].as_array().unwrap();
    assert!(
        glob_matches
            .iter()
            .any(|item| item.as_str() == Some(alpha.to_string_lossy().as_ref()))
    );
    assert!(
        glob_matches
            .iter()
            .any(|item| item.as_str() == Some(beta.to_string_lossy().as_ref()))
    );

    let grep = run_tool(
        &temp,
        &config,
        "grep",
        json!({"root": root.display().to_string(), "pattern": "needle", "limit": 10}),
    );
    let grep_matches = grep["output"]["matches"].as_array().unwrap();
    assert_eq!(grep_matches.len(), 1);
    assert_eq!(
        grep_matches[0]["path"].as_str(),
        Some(alpha.to_string_lossy().as_ref())
    );

    let web_fetch = run_tool(
        &temp,
        &config,
        "web_fetch",
        json!({
            "url": "https://example.test/page",
            "_stub": { "web_fetch": { "https://example.test/page": "fetched-page" } }
        }),
    );
    assert_eq!(
        web_fetch["output"]["content"].as_str(),
        Some("fetched-page")
    );

    let web_search = run_tool(
        &temp,
        &config,
        "web_search",
        json!({
            "query": "rust sqlite",
            "_stub": {
                "web_search": {
                    "rust sqlite": [
                        {"title": "Result One", "url": "https://example.test/1"},
                        {"title": "Result Two", "url": "https://example.test/2"}
                    ]
                }
            }
        }),
    );
    assert_eq!(
        web_search["output"]["provider"].as_str(),
        Some("duckduckgo_html")
    );
    assert_eq!(web_search["output"]["results"].as_array().unwrap().len(), 2);

    let mut brave_config: Value =
        serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    brave_config["search"]["provider"] = Value::String("brave_api".into());
    fs::write(
        &config,
        serde_json::to_string_pretty(&brave_config).unwrap(),
    )
    .unwrap();
    let brave_search = run_tool(
        &temp,
        &config,
        "web_search",
        json!({
            "query": "rust sqlite",
            "_stub": {
                "web_search": {
                    "rust sqlite": [
                        {"title": "Brave Result", "url": "https://example.test/brave"}
                    ]
                }
            }
        }),
    );
    assert_eq!(
        brave_search["output"]["provider"].as_str(),
        Some("brave_api")
    );
    assert_eq!(
        brave_search["output"]["results"].as_array().unwrap().len(),
        1
    );

    let file_write_path = root.join("written.txt");
    let file_write = run_tool(
        &temp,
        &config,
        "file_write",
        json!({"path": file_write_path.display().to_string(), "content": "written content"}),
    );
    assert_eq!(file_write["output"]["bytes"].as_u64(), Some(15));
    assert_eq!(
        fs::read_to_string(&file_write_path).unwrap(),
        "written content"
    );

    let file_edit = run_tool(
        &temp,
        &config,
        "file_edit",
        json!({"path": file_write_path.display().to_string(), "find": "written", "replace": "updated"}),
    );
    assert_eq!(file_edit["output"]["replacements"].as_u64(), Some(1));
    assert_eq!(
        fs::read_to_string(&file_write_path).unwrap(),
        "updated content"
    );

    let shell_exec = run_tool(
        &temp,
        &config,
        "shell_exec",
        json!({"command": "printf shell-ok", "cwd": root.display().to_string()}),
    );
    assert_eq!(shell_exec["output"]["status"].as_i64(), Some(0));
    assert_eq!(shell_exec["output"]["stdout"].as_str(), Some("shell-ok"));

    let interactive_batch = run_tool_batch(
        &temp,
        &config,
        json!([
            {
                "tool": "interactive_shell_start",
                "args": {
                    "command": "printf 'name? '; read name; printf 'hi:%s' $name",
                    "cwd": root.display().to_string()
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "wait_ms": 500
                }
            },
            {
                "tool": "interactive_shell_write",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "text": "alex",
                    "submit": true
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "since_seq": "__FROM_RESULT_1__.output.end_seq",
                    "wait_ms": 500
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "since_seq": "__FROM_RESULT_3__.output.end_seq",
                    "wait_ms": 500
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "since_seq": "__FROM_RESULT_4__.output.end_seq",
                    "wait_ms": 500
                }
            },
            {
                "tool": "interactive_shell_close",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id"
                }
            }
        ]),
    );
    let results = interactive_batch["results"].as_array().unwrap();
    let initial_read = &results[1];
    assert!(
        initial_read["output"]["output"]
            .as_str()
            .unwrap_or("")
            .contains("name?")
    );
    let idle_read = run_tool_batch(
        &temp,
        &config,
        json!([
            {
                "tool": "interactive_shell_start",
                "args": {
                    "command": "printf 'prompt: '; sleep 1",
                    "cwd": root.display().to_string()
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "wait_ms": 500,
                    "settle_ms": 250
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "since_seq": "__FROM_RESULT_1__.output.end_seq",
                    "wait_ms": 500,
                    "settle_ms": 250
                }
            },
            {
                "tool": "interactive_shell_close",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id"
                }
            }
        ]),
    );
    let idle_results = idle_read["results"].as_array().unwrap();
    assert_eq!(idle_results[2]["output"]["output"].as_str(), Some(""));
    assert_eq!(
        idle_results[2]["output"]["display_output"].as_str(),
        Some("")
    );
    assert!(
        idle_results[2]["output"]["screen_text"]
            .as_str()
            .unwrap_or("")
            .contains("prompt:")
    );
    let interactive_shell_write = &results[2];
    assert_eq!(
        interactive_shell_write["output"]["bytes_written"].as_u64(),
        Some(5)
    );
    assert_eq!(
        interactive_shell_write["output"]["submitted"].as_bool(),
        Some(true)
    );
    let final_read = format!(
        "{}{}{}",
        results[3]["output"]["output"].as_str().unwrap_or(""),
        results[4]["output"]["output"].as_str().unwrap_or(""),
        results[5]["output"]["output"].as_str().unwrap_or("")
    );
    assert!(final_read.contains("hi:alex"));
    let interactive_shell_close = &results[6];
    assert_eq!(
        interactive_shell_close["output"]["closed"].as_bool(),
        Some(true)
    );

    let raw_enter_batch = run_tool_batch(
        &temp,
        &config,
        json!([
            {
                "tool": "interactive_shell_start",
                "args": {
                    "command": "python3 -c 'import sys, tty, termios; print(\"ready\", flush=True); fd=sys.stdin.fileno(); old=termios.tcgetattr(fd); tty.setraw(fd); b=sys.stdin.buffer.read(1); print(b.hex(), flush=True); termios.tcsetattr(fd, termios.TCSANOW, old)'",
                    "cwd": root.display().to_string()
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "wait_ms": 4000,
                    "settle_ms": 250
                }
            },
            {
                "tool": "interactive_shell_key",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "key": "Enter"
                }
            },
            {
                "tool": "interactive_shell_read",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id",
                    "since_seq": "__FROM_RESULT_1__.output.end_seq",
                    "wait_ms": 1000
                }
            },
            {
                "tool": "interactive_shell_close",
                "args": {
                    "session_id": "__FROM_RESULT_0__.output.session_id"
                }
            }
        ]),
    );
    let raw_results = raw_enter_batch["results"].as_array().unwrap();
    assert!(
        raw_results[1]["output"]["output"]
            .as_str()
            .unwrap_or("")
            .contains("ready"),
        "expected raw-mode helper to signal readiness, got:\n{}",
        raw_results[1]
    );
    assert_eq!(raw_results[2]["output"]["bytes_written"].as_u64(), Some(1));
    assert_eq!(raw_results[2]["output"]["key"].as_str(), Some("Enter"));
    assert!(
        raw_results[3]["output"]["output"]
            .as_str()
            .unwrap_or("")
            .contains("0d"),
        "expected submit=true to send carriage return, got:\n{}",
        raw_results[3]
    );

    let mcp_tools_list = run_tool(&temp, &config, "mcp_tools_list", json!({"server": "docs"}));
    let mcp_tools = mcp_tools_list["output"]["tools"].as_array().unwrap();
    assert!(
        mcp_tools
            .iter()
            .any(|item| item["name"].as_str() == Some("echo"))
    );
    assert!(
        mcp_tools
            .iter()
            .any(|item| item["name"].as_str() == Some("counter"))
    );

    let mcp_resources_list = run_tool(
        &temp,
        &config,
        "mcp_resources_list",
        json!({"server": "docs"}),
    );
    let mcp_resources = mcp_resources_list["output"]["resources"]
        .as_array()
        .unwrap();
    assert!(
        mcp_resources
            .iter()
            .any(|item| item["uri"].as_str() == Some("memory://summary"))
    );

    let mcp_resource_read = run_tool(
        &temp,
        &config,
        "mcp_resource_read",
        json!({"server": "docs", "uri": "memory://summary"}),
    );
    assert_eq!(
        mcp_resource_read["output"]["content"].as_str(),
        Some("memory summary")
    );

    let mcp_tool_batch = run_tool_batch(
        &temp,
        &config,
        json!([
            {
                "tool": "mcp_tool_call",
                "args": {
                    "server": "docs",
                    "tool": "echo",
                    "arguments": { "text": "hello" }
                }
            },
            {
                "tool": "mcp_tool_call",
                "args": {
                    "server": "docs",
                    "tool": "counter",
                    "arguments": {}
                }
            },
            {
                "tool": "mcp_tool_call",
                "args": {
                    "server": "docs",
                    "tool": "counter",
                    "arguments": {}
                }
            }
        ]),
    );
    let mcp_tool_results = mcp_tool_batch["results"].as_array().unwrap();
    assert_eq!(
        mcp_tool_results[0]["output"]["content_text"].as_str(),
        Some("echo:hello")
    );
    assert_eq!(
        mcp_tool_results[1]["output"]["content_text"].as_str(),
        Some("counter:1")
    );
    assert_eq!(
        mcp_tool_results[2]["output"]["content_text"].as_str(),
        Some("counter:2")
    );

    let mcp_tools_list_ndjson = run_tool(
        &temp,
        &config,
        "mcp_tools_list",
        json!({"server": "docs_ndjson"}),
    );
    let mcp_tools_ndjson = mcp_tools_list_ndjson["output"]["tools"].as_array().unwrap();
    assert!(
        mcp_tools_ndjson
            .iter()
            .any(|item| item["name"].as_str() == Some("echo"))
    );

    let mcp_tool_call_ndjson = run_tool(
        &temp,
        &config,
        "mcp_tool_call",
        json!({
            "server": "docs_ndjson",
            "tool": "echo",
            "arguments": { "text": "ndjson" }
        }),
    );
    assert_eq!(
        mcp_tool_call_ndjson["output"]["content_text"].as_str(),
        Some("echo:ndjson")
    );

    let task_note_write = run_tool(
        &temp,
        &config,
        "task_note_write",
        json!({"kind": "next_steps", "content": "ship noodle"}),
    );
    assert_eq!(task_note_write["output"]["written"].as_bool(), Some(true));

    let agent_handoff_create = run_tool(
        &temp,
        &config,
        "agent_handoff_create",
        json!({"agent": "planner", "content": "remember this"}),
    );
    assert_eq!(
        agent_handoff_create["output"]["written"].as_bool(),
        Some(true)
    );

    let memory_query = run_tool(
        &temp,
        &config,
        "memory_query",
        json!({"source": "artifacts", "limit": 10}),
    );
    let artifacts = memory_query["output"]["artifacts"].as_array().unwrap();
    assert!(
        artifacts
            .iter()
            .any(|item| item["plugin"].as_str() == Some("tasks")
                && item["kind"].as_str() == Some("next_steps"))
    );
    assert!(
        artifacts
            .iter()
            .any(|item| item["plugin"].as_str() == Some("agents")
                && item["kind"].as_str() == Some("handoff:planner"))
    );
}

#[test]
fn chat_tool_harness_prints_model_selected_tools_and_results() {
    let temp = TempDir::new("noodle-e2e-chat-tool-harness");
    let config = write_test_config(&temp, "auto");
    set_config_permissions_allow(&config);

    let scenarios = vec![
        ("oo find me the README file in this repo", "path_search"),
        (
            "oo show me the contents of README.md in this directory",
            "file_read",
        ),
        ("oo list every txt file under the harness folder", "glob"),
        ("oo search the harness folder for the word needle", "grep"),
        ("oo fetch https://example.test/page", "web_fetch"),
        ("oo search the web for rust sqlite", "web_search"),
        (
            "oo write a file named written-by-harness.txt in the harness folder with the text written by harness",
            "file_write",
        ),
        (
            "oo replace before with after in harness/edit-target.txt",
            "file_edit",
        ),
        (
            "oo run printf harness-shell-ok in the harness folder",
            "shell_exec",
        ),
        ("oo list the MCP tools on docs", "mcp_tools_list"),
        ("oo call the echo tool on docs with hello", "mcp_tool_call"),
        (
            "oo read the MCP memory summary resource from docs",
            "mcp_resource_read",
        ),
        (
            "oo save a task note called harness_note saying ship noodle",
            "task_note_write",
        ),
        (
            "oo create an agent handoff for planner saying remember this",
            "agent_handoff_create",
        ),
        (
            "oo show me the current task artifacts in noodle memory",
            "memory_query",
        ),
    ];

    for (request, expected_tool) in scenarios {
        let result = run_mode(&temp, &config, "command_not_found", request, None);
        assert_eq!(result["action"].as_str(), Some("batch"));
        let task_id = task_id_from_batch(&result);
        let show = run_task_command(&temp, &config, "task-show", &["--task-id", &task_id]);
        let step = task_step_from_show(&show);
        assert_eq!(step["tool"].as_str(), Some(expected_tool));
        assert_eq!(step["status"].as_str(), Some("done"));
        println!("\n== {}", request);
        println!("tool: {}", step["tool"].as_str().unwrap());
        println!(
            "result: {}",
            serde_json::to_string_pretty(&step["result"]).unwrap()
        );
    }

    assert_eq!(
        fs::read_to_string(temp.path().join("harness/written-by-harness.txt")).unwrap(),
        "written by harness"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("harness/edit-target.txt")).unwrap(),
        "after"
    );
}
