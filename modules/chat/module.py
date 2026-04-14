#!/usr/bin/env python3
import json
import os
import subprocess
import sys

CHAT_PLUGIN = "chat"
DEFAULT_CHAT_PROMPT = (
    "You are noodle, a local terminal agent. You help the user think, search, "
    "inspect files, read and edit code, run commands, and complete tasks using "
    "the tools available to you. You are workspace-aware when relevant, but "
    "not limited to software engineering or zsh. Be concise, practical, and "
    "action-oriented."
)
DEFAULT_SOUL = (
    "You are noodle, a concise, helpful, calm, and direct zsh assistant. "
    "You live inside the user's terminal and answer briefly in plain text. "
    "Do not be theatrical or verbose."
)


def respond(ok, payload=None, error=None, stream=False):
    if stream:
        envelope = {
            "type": "final" if ok else "error",
            "ok": ok,
            "payload": payload,
            "error": error,
        }
        sys.stdout.write(json.dumps(envelope) + "\n")
    else:
        sys.stdout.write(json.dumps({"ok": ok, "payload": payload, "error": error}))
    sys.stdout.flush()


def resolved_config_path(request):
    return os.path.expandvars(
        os.path.expanduser(
            request.get("config_path")
            or os.environ.get("NOODLE_CONFIG")
            or "~/.noodle/config.json"
        )
    )


def host_binary(request):
    return (
        ((request.get("host") or {}).get("binary_path"))
        or os.environ.get("NOODLE_HELPER")
        or "noodle"
    )


def host_env():
    env = os.environ.copy()
    env["NOODLE_BYPASS_DAEMON"] = "1"
    return env


def run_host(request, args, input_text=None):
    command = [host_binary(request)] + args
    result = subprocess.run(
        command,
        input=input_text,
        text=True,
        capture_output=True,
        env=host_env(),
        check=False,
    )
    if result.returncode == 0:
        return result.stdout
    detail = result.stderr.strip() or result.stdout.strip() or "host command failed"
    raise RuntimeError(detail)


def stream_host(request, args, input_text, on_line):
    command = [host_binary(request)] + args
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=host_env(),
    )
    assert process.stdin is not None
    process.stdin.write(input_text)
    process.stdin.close()
    assert process.stdout is not None
    for line in process.stdout:
        line = line.rstrip("\n")
        if not line.strip():
            continue
        on_line(line)
    stderr_text = ""
    if process.stderr is not None:
        stderr_text = process.stderr.read()
    return_code = process.wait()
    if return_code != 0:
        detail = stderr_text.strip() or "host streaming command failed"
        raise RuntimeError(detail)


def plugin_config(config):
    return ((config.get("plugins") or {}).get(CHAT_PLUGIN) or {})


def chat_prefix(config):
    return plugin_config(config).get("prefix") or os.environ.get("NOODLE_CHAT_PREFIX") or ","


def chat_input(config, raw_input):
    prefix = chat_prefix(config)
    if raw_input == "oo":
        trimmed = ""
    elif raw_input.startswith("oo "):
        trimmed = raw_input[len("oo") :]
    elif raw_input.startswith(prefix):
        trimmed = raw_input[len(prefix) :]
    else:
        trimmed = raw_input
    return trimmed.lstrip()


def noodle_soul(config):
    soul = config.get("soul")
    if isinstance(soul, str) and soul.strip():
        return soul
    return DEFAULT_SOUL


def sanitize_chat_instructions(instructions):
    lines = []
    for line in instructions.splitlines():
        trimmed = line.strip()
        if not trimmed or "{user_input}" in trimmed:
            continue
        lines.append(trimmed)
    return "\n".join(lines)


def render_sections(sections):
    rendered = []
    for title, body in sections:
        body = body.strip()
        if body:
            rendered.append(f"[{title}]\n{body}")
    return "\n\n".join(rendered)


def join_sections(sections):
    return "\n\n".join(section.strip() for section in sections if section.strip())


def build_chat_base_prompt(instructions, user_input, cwd, shell, recent_command, soul, extra_sections):
    sections = []
    if soul and soul.strip():
        sections.append(("Identity", soul.strip()))
    sanitized = sanitize_chat_instructions(instructions)
    if sanitized:
        sections.append(("Operating Instructions", sanitized))
    runtime_lines = [f"Current directory: {cwd}", f"Shell: {shell}"]
    if recent_command.strip():
        runtime_lines.append(f"Recent command: {recent_command.strip()}")
    sections.append(("Runtime Context", "\n".join(runtime_lines)))
    workspace_context = join_sections(extra_sections)
    if workspace_context:
        sections.append(("Workspace Context", workspace_context))
    sections.append(("Current Request", user_input.strip()))
    return render_sections(sections)


def workspace_sections(request):
    stdout = run_host(
        request,
        ["workspace-context", "--cwd", request.get("cwd") or "."],
    )
    return json.loads(stdout or "[]")


def memory_context(request):
    return run_host(
        request,
        [
            "memory-context",
            "--config",
            resolved_config_path(request),
            "--plugin",
            CHAT_PLUGIN,
        ],
    )


def record_turns(request, user_text, payload):
    args = [
        "memory-record-turns",
        "--config",
        resolved_config_path(request),
        "--plugin",
        CHAT_PLUGIN,
        "--user-text",
        user_text,
        "--payload",
        json.dumps(payload, separators=(",", ":")),
    ]
    if request.get("debug"):
        args.append("--debug")
    run_host(request, args)


def execution_request(request):
    config = request.get("config") or {}
    user_input = chat_input(config, request.get("input") or "")
    instructions = (
        os.environ.get("NOODLE_CHAT_PROMPT")
        or plugin_config(config).get("prompt")
        or DEFAULT_CHAT_PROMPT
    )
    base_prompt = build_chat_base_prompt(
        instructions,
        user_input,
        request.get("cwd") or "",
        request.get("shell") or "zsh",
        request.get("recent_command") or "",
        noodle_soul(config),
        workspace_sections(request),
    )
    return {
        "plugin": CHAT_PLUGIN,
        "input": user_input,
        "working_directory": request.get("cwd") or "",
        "base_prompt": base_prompt,
        "memory_context": memory_context(request),
    }


def handle_command_not_found(request, stream):
    exec_request = execution_request(request)
    user_text = exec_request["input"]
    args = ["execution-run", "--config", resolved_config_path(request)]
    if request.get("debug"):
        args.append("--debug")
    payload_json = json.dumps(exec_request, separators=(",", ":"))
    if not stream:
        stdout = run_host(request, args, input_text=payload_json)
        payload = json.loads(stdout)
        record_turns(request, user_text, payload)
        return payload

    args.append("--stream")
    final_emitted = {"done": False}

    def on_line(line):
        envelope = json.loads(line)
        if envelope.get("type") == "final" and envelope.get("ok", True):
            payload = envelope.get("payload")
            if payload is not None:
                record_turns(request, user_text, payload)
            sys.stdout.write(json.dumps(envelope) + "\n")
            sys.stdout.flush()
            final_emitted["done"] = True
            return
        sys.stdout.write(json.dumps(envelope) + "\n")
        sys.stdout.flush()

    stream_host(request, args, payload_json, on_line)
    if not final_emitted["done"]:
        raise RuntimeError("chat execution stream ended without final payload")
    return None


def handle_permission_response(request, stream):
    args = [
        "execution-resume-permission",
        "--config",
        resolved_config_path(request),
        "--permission-id",
        request.get("input") or "",
        "--decision",
        request.get("selected_command") or "",
    ]
    if request.get("debug"):
        args.append("--debug")
    if not stream:
        stdout = run_host(request, args)
        return json.loads(stdout)

    args.append("--stream")
    stream_host(
        request,
        args,
        "",
        lambda line: (
            sys.stdout.write(json.dumps(json.loads(line)) + "\n"),
            sys.stdout.flush(),
        ),
    )
    return None


def main():
    request = json.load(sys.stdin)
    stream = bool(request.get("stream"))
    event = request.get("event")
    try:
        if event == "command_not_found":
            payload = handle_command_not_found(request, stream)
        elif event == "permission_response":
            payload = handle_permission_response(request, stream)
        else:
            raise ValueError(f"unsupported chat event: {event}")
        if not stream:
            respond(True, payload, stream=False)
    except Exception as exc:
        respond(False, error=str(exc), stream=stream)


if __name__ == "__main__":
    main()
