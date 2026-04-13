use portable_pty::{Child as PtyChild, CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use vt100::Parser;

#[derive(Debug, Clone)]
struct OutputChunk {
    seq: u64,
    text: String,
}

struct SessionState {
    command: String,
    cwd: Option<String>,
    chunks: Vec<OutputChunk>,
    next_seq: u64,
    closed: bool,
    exit_status: Option<i32>,
    parser: Parser,
    last_output_at: Option<Instant>,
}

#[derive(Clone)]
struct InteractiveShellSession {
    stdin: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    child: Arc<Mutex<Option<Box<dyn PtyChild + Send + Sync>>>>,
    state: Arc<(Mutex<SessionState>, Condvar)>,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, InteractiveShellSession>>> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<String, InteractiveShellSession>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("shell-{nanos}")
}

fn append_output(state: &Arc<(Mutex<SessionState>, Condvar)>, text: String) {
    if text.is_empty() {
        return;
    }
    let (lock, condvar) = &**state;
    let mut guard = lock.lock().unwrap();
    guard.parser.process(text.as_bytes());
    let seq = guard.next_seq.saturating_add(1);
    guard.next_seq = seq;
    guard.chunks.push(OutputChunk { seq, text });
    guard.last_output_at = Some(Instant::now());
    condvar.notify_all();
}

fn mark_closed(state: &Arc<(Mutex<SessionState>, Condvar)>, exit_status: Option<i32>) {
    let (lock, condvar) = &**state;
    let mut guard = lock.lock().unwrap();
    guard.closed = true;
    guard.exit_status = exit_status;
    condvar.notify_all();
}

fn spawn_reader_thread<R>(mut reader: R, state: Arc<(Mutex<SessionState>, Condvar)>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    append_output(
                        &state,
                        String::from_utf8_lossy(&buffer[..count]).into_owned(),
                    );
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_monitor_thread(
    child: Arc<Mutex<Option<Box<dyn PtyChild + Send + Sync>>>>,
    state: Arc<(Mutex<SessionState>, Condvar)>,
) {
    thread::spawn(move || {
        loop {
            let status = {
                let mut guard = child.lock().unwrap();
                let Some(process) = guard.as_mut() else {
                    mark_closed(&state, None);
                    return;
                };
                match process.try_wait() {
                    Ok(Some(status)) => Some(status.exit_code() as i32),
                    Ok(None) => None,
                    Err(_) => Some(-1),
                }
            };
            if let Some(code) = status {
                mark_closed(&state, Some(code));
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
    });
}

fn session_by_id(session_id: &str) -> Result<InteractiveShellSession, String> {
    sessions()
        .lock()
        .unwrap()
        .get(session_id)
        .cloned()
        .ok_or_else(|| format!("interactive shell session not found: {session_id}"))
}

fn default_pty_size() -> PtySize {
    PtySize {
        rows: 60,
        cols: 180,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn build_session_command(command: &str, cwd: Option<&str>) -> CommandBuilder {
    let mut builder = CommandBuilder::new("/bin/zsh");
    builder.arg("-lc");
    builder.arg(command);
    if let Some(path) = cwd {
        builder.cwd(path);
    }
    builder
}

fn prompt_like_line(line: &str) -> bool {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return false;
    }
    let prompt_markers = ['>', '$', '#', '%', ':', '?', '❯', '›'];
    let last = trimmed.chars().last().unwrap_or_default();
    if prompt_markers.contains(&last) {
        return true;
    }
    let lowered = trimmed.to_lowercase();
    lowered.ends_with("press enter")
        || lowered.ends_with("[y/n]")
        || lowered.ends_with("[y/n]:")
        || lowered.ends_with("[y/n]?")
        || lowered.ends_with("(y/n)")
        || lowered.ends_with("(y/n):")
        || lowered.ends_with("(yes/no)")
        || lowered.ends_with("password:")
        || lowered.ends_with("passphrase:")
        || lowered.ends_with("login:")
}

fn actionable_menu_lines(lines: &[&str]) -> bool {
    let numbered = lines
        .iter()
        .filter(|line| {
            let trimmed = line.trim_start();
            let mut chars = trimmed.chars();
            let Some(first) = chars.next() else {
                return false;
            };
            first.is_ascii_digit() && matches!(chars.next(), Some('.' | ')' | ':'))
        })
        .count();
    if numbered >= 2 {
        return true;
    }
    let joined = lines.join("\n").to_lowercase();
    joined.contains("do you want to proceed")
        || joined.contains("choose [")
        || joined.contains("select an option")
        || joined.contains("this command requires approval")
        || joined.contains("do you want to continue")
        || joined.contains("allow [y/n]")
        || joined.contains("allow [y/n]:")
        || joined.contains("allow [y/n]?")
        || joined.contains("allow [y/n")
        || joined.contains("yes, and don")
}

fn detect_prompt(screen_text: &str) -> bool {
    let lines = screen_text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return false;
    }
    let tail = lines
        .iter()
        .rev()
        .take(8)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    tail.last().copied().map(prompt_like_line).unwrap_or(false) || actionable_menu_lines(&tail)
}

fn screen_tail(screen_text: &str, max_lines: usize, max_chars: usize) -> String {
    if screen_text.trim().is_empty() {
        return String::new();
    }
    let lines = screen_text.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max_lines);
    let mut tail = lines[start..].join("\n");
    let char_count = tail.chars().count();
    if char_count > max_chars {
        let keep = tail
            .chars()
            .rev()
            .take(max_chars)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        tail = format!("...[truncated]\n{keep}");
    }
    tail
}

fn prompt_region(screen_text: &str, max_lines: usize, max_chars: usize) -> String {
    let lines = screen_text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }
    let start = lines.len().saturating_sub(max_lines);
    let mut region = lines[start..].join("\n");
    let char_count = region.chars().count();
    if char_count > max_chars {
        let keep = region
            .chars()
            .rev()
            .take(max_chars)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        region = format!("...[truncated]\n{keep}");
    }
    region
}

fn menu_options(screen_text: &str, max_lines: usize) -> Vec<String> {
    screen_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(max_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter(|line| {
            let normalized = line
                .trim_start_matches(|ch: char| {
                    ch.is_whitespace()
                        || matches!(ch, '>' | '*' | '-' | '+' | '•' | '◦' | '▪' | '❯' | '›')
                })
                .trim_start();
            let mut chars = normalized.chars();
            let Some(first) = chars.next() else {
                return false;
            };
            first.is_ascii_digit() && matches!(chars.next(), Some('.' | ')' | ':'))
        })
        .map(ToOwned::to_owned)
        .collect()
}

fn consume_csi(bytes: &[u8], start: usize) -> (usize, bool) {
    let mut i = start + 2;
    while i < bytes.len() {
        let byte = bytes[i];
        if (0x40..=0x7e).contains(&byte) {
            return (i + 1, byte == b'm');
        }
        i += 1;
    }
    (bytes.len(), false)
}

fn csi_is_safe_for_display(bytes: &[u8], start: usize, end: usize) -> bool {
    if end <= start + 2 {
        return false;
    }
    let final_byte = bytes[end - 1];
    matches!(final_byte, b'm')
}

fn consume_until_st(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 2;
    while i < bytes.len() {
        match bytes[i] {
            0x07 => return i + 1,
            0x1b if i + 1 < bytes.len() && bytes[i + 1] == b'\\' => return i + 2,
            _ => i += 1,
        }
    }
    bytes.len()
}

#[derive(Clone, Copy)]
enum TerminalRenderMode {
    Plain,
    Display,
}

fn sanitize_terminal_output(input: &str, mode: TerminalRenderMode) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b => {
                if i + 1 >= bytes.len() {
                    break;
                }
                match bytes[i + 1] {
                    b'[' => {
                        let (next, keep_sequence) = consume_csi(bytes, i);
                        let keep_for_display = matches!(mode, TerminalRenderMode::Display)
                            && csi_is_safe_for_display(bytes, i, next);
                        if keep_sequence || keep_for_display {
                            output.push_str(&String::from_utf8_lossy(&bytes[i..next]));
                        }
                        i = next;
                    }
                    b']' | b'P' | b'X' | b'^' | b'_' => {
                        i = consume_until_st(bytes, i);
                    }
                    _ => {
                        i = (i + 2).min(bytes.len());
                    }
                }
            }
            b'\r' => {
                if matches!(mode, TerminalRenderMode::Display) {
                    output.push('\r');
                }
                i += 1;
            }
            byte if byte < 0x20 && byte != b'\n' && byte != b'\t' => {
                i += 1;
            }
            _ => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    let byte = bytes[i];
                    if byte == 0x1b
                        || byte == b'\r'
                        || (byte < 0x20 && byte != b'\n' && byte != b'\t')
                    {
                        break;
                    }
                    i += 1;
                }
                output.push_str(&String::from_utf8_lossy(&bytes[start..i]));
            }
        }
    }
    output
}

pub fn interactive_shell_start(args: &Value) -> Result<Value, String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| "interactive_shell_start requires command".to_string())?;
    let cwd = args
        .get("cwd")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(default_pty_size())
        .map_err(|err| err.to_string())?;
    let cmd = build_session_command(command, cwd.as_deref());
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| err.to_string())?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| err.to_string())?;
    let stdin = pair.master.take_writer().map_err(|err| err.to_string())?;
    drop(pair.slave);

    let session_id = next_session_id();
    let state = Arc::new((
        Mutex::new(SessionState {
            command: command.to_string(),
            cwd: cwd.clone(),
            chunks: Vec::new(),
            next_seq: 0,
            closed: false,
            exit_status: None,
            parser: Parser::new(60, 180, 10_000),
            last_output_at: None,
        }),
        Condvar::new(),
    ));
    let child = Arc::new(Mutex::new(Some(child)));

    spawn_reader_thread(reader, state.clone());
    spawn_monitor_thread(child.clone(), state.clone());

    sessions().lock().unwrap().insert(
        session_id.clone(),
        InteractiveShellSession {
            stdin: Arc::new(Mutex::new(Some(stdin))),
            child,
            state,
        },
    );

    Ok(json!({
        "session_id": session_id,
        "status": "running",
        "command": command,
        "cwd": cwd,
    }))
}

pub fn interactive_shell_read(args: &Value) -> Result<Value, String> {
    let session_id = args
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "interactive_shell_read requires session_id".to_string())?;
    let since_seq = args.get("since_seq").and_then(Value::as_u64).unwrap_or(0);
    let wait_ms = args
        .get("wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1_000)
        .min(10_000);
    let settle_ms = args
        .get("settle_ms")
        .and_then(Value::as_u64)
        .unwrap_or(350)
        .min(wait_ms.max(1));
    let max_chars = args
        .get("max_chars")
        .and_then(Value::as_u64)
        .unwrap_or(8192)
        .clamp(256, 65_536) as usize;
    let session = session_by_id(session_id)?;
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let (lock, condvar) = &*session.state;
    let mut guard = lock.lock().unwrap();

    while !guard.closed && !guard.chunks.iter().any(|chunk| chunk.seq > since_seq) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if remaining.is_zero() {
            break;
        }
        let (next_guard, timeout) = condvar.wait_timeout(guard, remaining).unwrap();
        guard = next_guard;
        if timeout.timed_out() {
            break;
        }
    }

    let mut last_seen_seq = guard
        .chunks
        .iter()
        .filter(|chunk| chunk.seq > since_seq)
        .map(|chunk| chunk.seq)
        .max()
        .unwrap_or(since_seq);
    if last_seen_seq > since_seq && !guard.closed {
        let mut quiet_deadline = Instant::now() + Duration::from_millis(settle_ms);
        loop {
            let Some(total_remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let Some(quiet_remaining) = quiet_deadline.checked_duration_since(Instant::now())
            else {
                break;
            };
            let remaining = total_remaining.min(quiet_remaining);
            if remaining.is_zero() {
                break;
            }
            let (next_guard, timeout) = condvar.wait_timeout(guard, remaining).unwrap();
            guard = next_guard;
            let newest_seq = guard
                .chunks
                .iter()
                .filter(|chunk| chunk.seq > since_seq)
                .map(|chunk| chunk.seq)
                .max()
                .unwrap_or(last_seen_seq);
            if newest_seq > last_seen_seq {
                last_seen_seq = newest_seq;
                quiet_deadline = Instant::now() + Duration::from_millis(settle_ms);
                continue;
            }
            if timeout.timed_out() || guard.closed {
                break;
            }
        }
    }

    let mut output = String::new();
    let mut end_seq = since_seq;
    for chunk in guard.chunks.iter().filter(|chunk| chunk.seq > since_seq) {
        if output.len() >= max_chars {
            break;
        }
        let remaining = max_chars.saturating_sub(output.len());
        if chunk.text.len() <= remaining {
            output.push_str(&chunk.text);
        } else {
            output.push_str(&chunk.text[..remaining]);
        }
        end_seq = chunk.seq;
    }

    let screen = guard.parser.screen();
    let screen_text = screen.contents();
    let screen_formatted = String::from_utf8_lossy(&screen.contents_formatted()).into_owned();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let prompt_detected = detect_prompt(&screen_text);
    let prompt_region = prompt_region(&screen_text, 16, 2_000);
    let screen_tail = screen_tail(&screen_text, 32, 4_000);
    let menu_options = menu_options(&screen_text, 20);
    let idle_ms = guard
        .last_output_at
        .map(|instant| instant.elapsed().as_millis() as u64)
        .unwrap_or(wait_ms);

    let had_new_output = end_seq > since_seq;
    let display_output = if had_new_output {
        if !screen_text.trim().is_empty() {
            sanitize_terminal_output(&screen_formatted, TerminalRenderMode::Display)
        } else {
            sanitize_terminal_output(&output, TerminalRenderMode::Display)
        }
    } else {
        String::new()
    };
    let plain_output = if had_new_output {
        if !screen_text.trim().is_empty() {
            sanitize_terminal_output(&output, TerminalRenderMode::Plain)
        } else {
            sanitize_terminal_output(&display_output, TerminalRenderMode::Plain)
        }
    } else {
        String::new()
    };

    Ok(json!({
        "session_id": session_id,
        "command": guard.command,
        "cwd": guard.cwd,
        "output": plain_output,
        "display_output": display_output,
        "screen_text": screen_text,
        "screen_tail": screen_tail,
        "prompt_region": prompt_region,
        "menu_options": menu_options,
        "prompt_detected": prompt_detected,
        "had_new_output": had_new_output,
        "alternate_screen": screen.alternate_screen(),
        "cursor_hidden": screen.hide_cursor(),
        "cursor_row": cursor_row,
        "cursor_col": cursor_col,
        "idle_ms": idle_ms,
        "start_seq": since_seq,
        "end_seq": end_seq,
        "closed": guard.closed,
        "exit_status": guard.exit_status,
        "status": if guard.closed { "closed" } else { "running" },
    }))
}

pub fn interactive_shell_write(args: &Value) -> Result<Value, String> {
    let session_id = args
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "interactive_shell_write requires session_id".to_string())?;
    let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| args.get("input").and_then(Value::as_str))
        .unwrap_or("");
    if text.is_empty() && !submit {
        return Err("interactive_shell_write requires text or submit".into());
    }
    let session = session_by_id(session_id)?;
    let (lock, _) = &*session.state;
    if lock.lock().unwrap().closed {
        return Err("interactive shell session is already closed".into());
    }
    let mut stdin_guard = session.stdin.lock().unwrap();
    let Some(stdin) = stdin_guard.as_mut() else {
        return Err("interactive shell session stdin is unavailable".into());
    };
    let mut payload = text.as_bytes().to_vec();
    if submit {
        payload.push(b'\r');
    }
    stdin
        .write_all(&payload)
        .and_then(|_| stdin.flush())
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "session_id": session_id,
        "bytes_written": payload.len(),
        "text": text,
        "submitted": submit,
    }))
}

fn key_bytes(key: &str) -> Result<Vec<u8>, String> {
    let normalized = key.trim().to_lowercase();
    let bytes = match normalized.as_str() {
        "enter" | "return" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "escape" | "esc" => vec![0x1b],
        "backspace" => vec![0x7f],
        "space" => vec![b' '],
        "up" | "arrowup" => b"\x1b[A".to_vec(),
        "down" | "arrowdown" => b"\x1b[B".to_vec(),
        "right" | "arrowright" => b"\x1b[C".to_vec(),
        "left" | "arrowleft" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "delete" | "del" => b"\x1b[3~".to_vec(),
        "insert" | "ins" => b"\x1b[2~".to_vec(),
        _ => {
            if let Some(rest) = normalized.strip_prefix("ctrl+") {
                let mut chars = rest.chars();
                if let Some(ch) = chars.next() {
                    if chars.next().is_none() && ch.is_ascii_alphabetic() {
                        vec![(ch.to_ascii_uppercase() as u8) & 0x1f]
                    } else {
                        return Err(format!("unsupported interactive shell key: {key}"));
                    }
                } else {
                    return Err(format!("unsupported interactive shell key: {key}"));
                }
            } else {
                return Err(format!("unsupported interactive shell key: {key}"));
            }
        }
    };
    Ok(bytes)
}

pub fn interactive_shell_key(args: &Value) -> Result<Value, String> {
    let session_id = args
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "interactive_shell_key requires session_id".to_string())?;
    let key = args
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| "interactive_shell_key requires key".to_string())?;
    let repeat = args
        .get("repeat")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 32) as usize;
    let session = session_by_id(session_id)?;
    let (lock, _) = &*session.state;
    if lock.lock().unwrap().closed {
        return Err("interactive shell session is already closed".into());
    }
    let mut stdin_guard = session.stdin.lock().unwrap();
    let Some(stdin) = stdin_guard.as_mut() else {
        return Err("interactive shell session stdin is unavailable".into());
    };
    let bytes = key_bytes(key)?;
    let mut payload = Vec::with_capacity(bytes.len() * repeat);
    for _ in 0..repeat {
        payload.extend_from_slice(&bytes);
    }
    stdin
        .write_all(&payload)
        .and_then(|_| stdin.flush())
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "session_id": session_id,
        "key": key,
        "repeat": repeat,
        "bytes_written": payload.len(),
    }))
}

pub fn interactive_shell_close(args: &Value) -> Result<Value, String> {
    let session_id = args
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "interactive_shell_close requires session_id".to_string())?;
    let session = sessions()
        .lock()
        .unwrap()
        .remove(session_id)
        .ok_or_else(|| format!("interactive shell session not found: {session_id}"))?;

    let mut exit_status = None;
    if let Some(mut child) = session.child.lock().unwrap().take() {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status.exit_code() as i32);
            }
            Ok(None) => {
                let _ = child.kill();
                exit_status = child.wait().ok().map(|status| status.exit_code() as i32);
            }
            Err(_) => {}
        }
    }

    {
        let mut stdin = session.stdin.lock().unwrap();
        stdin.take();
    }
    mark_closed(&session.state, exit_status);

    Ok(json!({
        "session_id": session_id,
        "closed": true,
        "exit_status": exit_status,
    }))
}
