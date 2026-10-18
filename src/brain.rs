//! Claude Code is the brain: each voice command is one `claude -p` run, and its stream-json
//! output becomes notch events. The prompt goes in through stdin, never argv, so nothing the
//! user says can be read as a command-line flag.
//!
//! Model, effort and permissions are not passed as flags: Claude Code reads them from its own
//! settings files (user scope, then the workspace's voice scope). See `claude_settings.rs`.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

use crate::config::Config;

/// The permission tool served by `claude-notch mcp-approve` (see approve.rs).
pub const APPROVE_TOOL: &str = "mcp__notch__approve";
const VOICE_PROMPT: &str = include_str!("../assets/voice-prompt.md");
const STDERR_TAIL: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainEvent {
    Started {
        session_id: String,
    },
    Tool {
        label: String,
    },
    Text(String),
    Finished {
        text: String,
        is_error: bool,
        session_id: Option<String>,
        denied: usize,
    },
    /// Always the last event of a run; `error` is set when it ended without an answer.
    Exited {
        error: Option<String>,
    },
}

/// Claude Code session ids are UUIDs; anything else must not reach argv.
pub fn is_session_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// `forced_mode` overrides the permission mode from the settings files, for the one case
/// `claude_settings::forced_mode` allows. `chrome` carries the user's own `/chrome` choice,
/// which Claude Code does not apply to `-p` runs by itself.
pub fn args(resume: Option<&str>, exe: &Path, forced_mode: Option<&str>, chrome: bool) -> Vec<String> {
    let mcp = serde_json::json!({
        "mcpServers": { "notch": { "command": exe.to_string_lossy(), "args": ["mcp-approve"] } }
    });
    let mut a: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        // Whatever escapes the permission mode (an ask rule, auto mode pausing after repeated
        // blocks) comes to the notch instead of being silently denied.
        "--permission-prompt-tool",
        APPROVE_TOOL,
        "--mcp-config",
    ]
    .map(String::from)
    .into();
    a.push(mcp.to_string());
    a.extend(["--append-system-prompt".into(), VOICE_PROMPT.into()]);
    if let Some(mode) = forced_mode {
        a.extend(["--permission-mode".into(), mode.into()]);
    }
    if chrome {
        a.push("--chrome".into());
    }
    if let Some(id) = resume.filter(|id| is_session_id(id)) {
        a.extend(["--resume".into(), id.into()]);
    }
    a
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |f| f.to_string_lossy().into_owned())
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// One short line for the notch while a tool runs.
pub fn tool_label(name: &str, input: &Value) -> String {
    let field = |k: &str| input[k].as_str().unwrap_or_default();
    let label = match name {
        "Bash" => format!("$ {}", field("command").lines().next().unwrap_or_default()),
        "Read" => format!("Reading {}", file_name(field("file_path"))),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => format!("Editing {}", file_name(field("file_path"))),
        "WebSearch" => format!("Searching “{}”", field("query")),
        "WebFetch" => format!("Opening {}", host(field("url"))),
        "Glob" | "Grep" => "Searching files".into(),
        "Task" | "Agent" => format!("Agent: {}", field("description")),
        _ => name.rsplit("__").next().unwrap_or(name).to_string(),
    };
    truncate(&label, 60)
}

/// Everything the user needs to decide on a permission request, in full.
pub fn approval_detail(name: &str, input: &Value) -> String {
    let field = |k: &str| input[k].as_str().unwrap_or_default();
    let detail = match name {
        "Bash" => field("command").replace('\n', " ↵ "),
        "Read" | "Edit" | "MultiEdit" | "NotebookEdit" => field("file_path").to_string(),
        "Write" => format!("{} ({} lines)", field("file_path"), field("content").lines().count()),
        "WebFetch" => field("url").to_string(),
        "WebSearch" => field("query").to_string(),
        _ => input.to_string(),
    };
    truncate(&detail, 600)
}

pub fn parse_line(line: &str) -> Vec<BrainEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let string = |v: &Value| v.as_str().map(str::to_string);
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => {
            string(&v["session_id"]).map(|session_id| BrainEvent::Started { session_id }).into_iter().collect()
        }
        Some("assistant") => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| match block["type"].as_str() {
                Some("text") => {
                    block["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(|t| BrainEvent::Text(t.into()))
                }
                Some("tool_use") => Some(BrainEvent::Tool {
                    label: tool_label(block["name"].as_str().unwrap_or("tool"), &block["input"]),
                }),
                _ => None,
            })
            .collect(),
        Some("result") => vec![BrainEvent::Finished {
            text: v["result"].as_str().unwrap_or_default().trim().to_string(),
            is_error: v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success",
            session_id: string(&v["session_id"]),
            denied: v["permission_denials"].as_array().map_or(0, Vec::len),
        }],
        _ => Vec::new(),
    }
}

/// Runs one command to the end, cancellation or timeout. Dropping `cancel`'s sender cancels too.
/// The last event is always [`BrainEvent::Exited`].
pub async fn run(
    cfg: &Config,
    prompt: &str,
    resume: Option<&str>,
    cwd: &Path,
    exe: &Path,
    cancel: oneshot::Receiver<()>,
    mut emit: impl FnMut(BrainEvent),
) {
    let error = drive(cfg, prompt, resume, cwd, exe, cancel, &mut emit).await.err();
    emit(BrainEvent::Exited { error });
}

async fn drive(
    cfg: &Config,
    prompt: &str,
    resume: Option<&str>,
    cwd: &Path,
    exe: &Path,
    mut cancel: oneshot::Receiver<()>,
    emit: &mut impl FnMut(BrainEvent),
) -> Result<(), String> {
    let mut child = claude(cfg, cwd)
        .args(args(resume, exe, crate::claude_settings::forced_mode(cwd), crate::claude_settings::chrome_enabled()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", cfg.claude_bin))?;

    let (Some(mut stdin), Some(stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        stop(&mut child).await;
        return Err("claude's pipes are missing".into());
    };
    let stderr_tail = tokio::spawn(async move {
        let mut tail = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n @ 1..) = stderr.read(&mut chunk).await {
            tail.extend_from_slice(&chunk[..n]);
            let excess = tail.len().saturating_sub(STDERR_TAIL);
            tail.drain(..excess);
        }
        String::from_utf8_lossy(&tail).trim().to_string()
    });
    if let Err(e) = stdin.write_all(prompt.as_bytes()).await {
        stop(&mut child).await;
        return Err(format!("sending the prompt to claude: {e}"));
    }
    drop(stdin);

    let mut lines = BufReader::new(stdout).lines();
    let timeout = tokio::time::sleep(Duration::from_secs(cfg.turn_timeout_secs));
    tokio::pin!(timeout);
    let mut finished = false;
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    for event in parse_line(&line) {
                        finished |= matches!(event, BrainEvent::Finished { .. });
                        emit(event);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    stop(&mut child).await;
                    return Err(format!("reading claude's output: {e}"));
                }
            },
            _ = &mut cancel => {
                stop(&mut child).await;
                return Ok(());
            }
            () = &mut timeout => {
                stop(&mut child).await;
                return Err(format!("Claude took longer than {} s and was stopped", cfg.turn_timeout_secs));
            }
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let tail = stderr_tail.await.unwrap_or_default();
    if finished {
        Ok(())
    } else if tail.is_empty() {
        Err(format!("claude ended ({status}) without an answer"))
    } else {
        Err(truncate(&tail, 300))
    }
}

/// `claude` started in `cwd`, so the workspace's voice settings apply.
fn claude(cfg: &Config, cwd: &Path) -> Command {
    let mut command = Command::new(&cfg.claude_bin);
    command
        .current_dir(cwd)
        // Set when the notch itself was started from a Claude Code session.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT");
    command
}

/// The settings window's connectivity test.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Probe {
    pub version: Option<String>,
    pub ok: bool,
    pub reply: String,
    pub model: Option<String>,
    pub seconds: f64,
    pub cost_usd: f64,
    pub error: Option<String>,
}

/// Fills `probe` from `claude -p --output-format json`'s single result object.
fn read_probe(stdout: &str, probe: &mut Probe) {
    let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) else {
        probe.error = Some(truncate(stdout.trim(), 300));
        return;
    };
    probe.reply = v["result"].as_str().unwrap_or_default().trim().to_string();
    probe.model = v["modelUsage"].as_object().and_then(|m| m.keys().next().cloned());
    probe.cost_usd = v["total_cost_usd"].as_f64().unwrap_or_default();
    probe.ok = v["subtype"] == "success" && !v["is_error"].as_bool().unwrap_or(true);
    if !probe.ok {
        probe.error =
            Some(if probe.reply.is_empty() { "Claude answered with an error".into() } else { probe.reply.clone() });
    }
}

/// Asks Claude for one word from the voice workspace, with the voice settings, and no tools.
pub async fn probe(cfg: &Config, cwd: &Path) -> Probe {
    let mut probe = Probe::default();
    let version = claude(cfg, cwd).arg("--version").stdin(Stdio::null()).kill_on_drop(true).output();
    match tokio::time::timeout(Duration::from_secs(15), version).await {
        Ok(Ok(out)) if out.status.success() => {
            probe.version = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
        Ok(Ok(out)) => probe.error = Some(truncate(String::from_utf8_lossy(&out.stderr).trim(), 300)),
        Ok(Err(e)) => probe.error = Some(format!("cannot start {}: {e}", cfg.claude_bin)),
        Err(_) => probe.error = Some("claude --version did not answer in 15 s".into()),
    }
    if probe.error.is_some() {
        return probe;
    }
    let started = std::time::Instant::now();
    let mut command = claude(cfg, cwd);
    command.args(["-p", "--output-format", "json", "--tools", ""]);
    if let Some(mode) = crate::claude_settings::forced_mode(cwd) {
        command.args(["--permission-mode", mode]);
    }
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            probe.error = Some(format!("cannot start {}: {e}", cfg.claude_bin));
            return probe;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"Reply with exactly one word: OK").await;
    }
    match tokio::time::timeout(Duration::from_secs(90), child.wait_with_output()).await {
        Ok(Ok(out)) => {
            read_probe(&String::from_utf8_lossy(&out.stdout), &mut probe);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !probe.ok && !stderr.trim().is_empty() {
                probe.error = Some(truncate(stderr.trim(), 300));
            }
        }
        Ok(Err(e)) => probe.error = Some(e.to_string()),
        Err(_) => probe.error = Some("Claude did not answer in 90 s".into()),
    }
    probe.seconds = started.elapsed().as_secs_f64();
    probe
}

/// SIGTERM to claude and everything it started in its process group, SIGKILL after 2 s.
/// Apps it launched with `setsid -f` live in their own session and are left alone.
async fn stop(child: &mut Child) {
    signal_group(child, libc::SIGTERM);
    if tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_err() {
        signal_group(child, libc::SIGKILL);
        let _ = child.wait().await;
    }
}

fn signal_group(child: &Child, signal: libc::c_int) {
    if let Some(pid) = child.id().and_then(|p| libc::pid_t::try_from(p).ok()) {
        // SAFETY: killpg only sends a signal; `pid` leads the group created by `process_group(0)`.
        unsafe { libc::killpg(pid, signal) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn all(fixture: &str) -> Vec<BrainEvent> {
        fixture.lines().flat_map(parse_line).collect()
    }

    #[test]
    fn args_leave_model_effort_and_permissions_to_claude_codes_settings() {
        let a = args(Some("4700cef8-84e7-4458-a427-1d252586e54c"), Path::new("/opt/claude-notch"), None, false);
        let at = |flag: &str| a.iter().position(|x| x == flag).unwrap();
        for flag in ["--model", "--effort", "--permission-mode", "--allowedTools", "--settings"] {
            assert!(!a.contains(&flag.to_string()), "{flag} would override Claude Code's settings");
        }
        assert_eq!(a[at("--permission-prompt-tool") + 1], APPROVE_TOOL);
        assert_eq!(a[at("--resume") + 1], "4700cef8-84e7-4458-a427-1d252586e54c");
        assert!(a.iter().all(|x| !x.contains("dangerously")));
        let forced = args(None, Path::new("x"), Some("auto"), false);
        assert!(!a.contains(&"--chrome".to_string()));
        assert!(args(None, Path::new("x"), None, true).contains(&"--chrome".to_string()));
        assert_eq!(forced[forced.iter().position(|x| x == "--permission-mode").unwrap() + 1], "auto");
        let mcp: Value = serde_json::from_str(&a[at("--mcp-config") + 1]).unwrap();
        assert_eq!(mcp["mcpServers"]["notch"]["command"], "/opt/claude-notch");
        assert_eq!(mcp["mcpServers"]["notch"]["args"][0], "mcp-approve");
    }

    #[test]
    fn args_drop_suspicious_resume_ids() {
        for bad in [
            "--dangerously-skip-permissions",
            "",
            "a b",
            "../x",
            "4700cef8-84e7-4458-a427-1d252586e54",
            "-700cef8-84e7-4458-a427-1d252586e54c",
            "4700cef8x84e7-4458-a427-1d252586e54c",
        ] {
            assert!(!args(Some(bad), Path::new("x"), None, false).contains(&"--resume".to_string()));
        }
        assert!(!args(None, Path::new("x"), None, false).contains(&"--resume".to_string()));
    }

    #[test]
    fn parses_a_text_answer() {
        let events = all(include_str!("../tests/fixtures/text.jsonl"));
        assert_eq!(
            events.first(),
            Some(&BrainEvent::Started { session_id: "605cba6e-a0ee-4d14-8bde-41f281ccdf02".into() })
        );
        assert!(events.contains(&BrainEvent::Text("hi".into())));
        assert_eq!(
            events.last(),
            Some(&BrainEvent::Finished {
                text: "hi".into(),
                is_error: false,
                session_id: Some("605cba6e-a0ee-4d14-8bde-41f281ccdf02".into()),
                denied: 0
            })
        );
    }

    #[test]
    fn parses_tool_use() {
        let events = all(include_str!("../tests/fixtures/tool.jsonl"));
        assert!(events.contains(&BrainEvent::Tool { label: "$ setsid -f sleep 4321 >/dev/null 2>&1".into() }));
        assert!(matches!(events.last(), Some(BrainEvent::Finished { text, is_error: false, .. }) if text == "done"));
    }

    #[test]
    fn counts_denials() {
        for fixture in
            [include_str!("../tests/fixtures/denied.jsonl"), include_str!("../tests/fixtures/approval.jsonl")]
        {
            assert!(matches!(all(fixture).last(), Some(BrainEvent::Finished { denied: 1, is_error: false, .. })));
        }
    }

    #[test]
    fn a_missing_conversation_is_an_error_without_a_session_start() {
        let events = all(include_str!("../tests/fixtures/resume-missing.jsonl"));
        assert!(!events.iter().any(|e| matches!(e, BrainEvent::Started { .. })));
        assert!(matches!(events.last(), Some(BrainEvent::Finished { is_error: true, .. })));
    }

    #[test]
    fn errors_and_garbage() {
        assert_eq!(parse_line("not json"), Vec::new());
        assert_eq!(parse_line("{\"type\":\"rate_limit_event\"}"), Vec::new());
        let events = parse_line(r#"{"type":"result","subtype":"error_max_turns","is_error":false}"#);
        assert!(matches!(&events[..], [BrainEvent::Finished { is_error: true, session_id: None, .. }]));
    }

    #[test]
    fn labels_and_details() {
        let input = serde_json::json!({"command": "ls -la\nrm -rf x", "file_path": "/home/u/a/b.rs",
            "url": "https://www.youtube.com/watch?v=1", "query": "lofi", "content": "a\nb\nc"});
        assert_eq!(tool_label("Bash", &input), "$ ls -la");
        assert_eq!(tool_label("Edit", &input), "Editing b.rs");
        assert_eq!(tool_label("WebFetch", &input), "Opening www.youtube.com");
        assert_eq!(tool_label("mcp__github__create_issue", &input), "create_issue");
        assert_eq!(approval_detail("Bash", &input), "ls -la ↵ rm -rf x");
        assert_eq!(approval_detail("Write", &input), "/home/u/a/b.rs (3 lines)");
        assert_eq!(approval_detail("Edit", &input), "/home/u/a/b.rs");
        let long = "x".repeat(100);
        assert_eq!(tool_label("Bash", &serde_json::json!({ "command": long })).chars().count(), 60);
    }

    #[test]
    fn reads_a_probe_result() {
        let result = include_str!("../tests/fixtures/text.jsonl").lines().last().unwrap();
        let mut probe = Probe::default();
        read_probe(result, &mut probe);
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.reply, "hi");
        assert_eq!(probe.model.as_deref(), Some("claude-haiku-5-5"));
        assert!(probe.cost_usd > 0.0);

        let mut failed = Probe::default();
        read_probe(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in"}"#, &mut failed);
        assert_eq!((failed.ok, failed.error.as_deref()), (false, Some("Not logged in")));
        let mut garbage = Probe::default();
        read_probe("Invalid API key", &mut garbage);
        assert_eq!((garbage.ok, garbage.error.as_deref()), (false, Some("Invalid API key")));
    }

    #[tokio::test]
    async fn probe_runs_one_toolless_turn() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/text.jsonl");
        let fake = Fake::new(
            "probe",
            &format!(
                "if [ \"$1\" = --version ]; then echo '2.1.302 (Claude Code)'; exit 0; fi\n\
                 printf '%s\\n' \"$@\" > \"$DIR/args\"\ncat > \"$DIR/stdin\"\ntail -n 1 {fixture}"
            ),
        );
        let probe = probe(&fake.cfg(), &fake.dir).await;
        assert!(probe.ok, "{probe:?}");
        assert_eq!(probe.version.as_deref(), Some("2.1.302 (Claude Code)"));
        let argv = std::fs::read_to_string(fake.dir.join("args")).unwrap();
        assert_eq!(argv, "-p\n--output-format\njson\n--tools\n\n", "no tools, no model or permission flags");
        assert_eq!(std::fs::read_to_string(fake.dir.join("stdin")).unwrap(), "Reply with exactly one word: OK");
    }

    #[tokio::test]
    async fn probe_reports_why_claude_failed() {
        let fake = Fake::new(
            "probe-fail",
            "if [ \"$1\" = --version ]; then echo 2.1.302; exit 0; fi\ncat >/dev/null\necho 'Not logged in · Please run /login' >&2\nexit 1",
        );
        let probe = probe(&fake.cfg(), &fake.dir).await;
        assert!(!probe.ok);
        assert_eq!(probe.error.as_deref(), Some("Not logged in · Please run /login"));
        let missing = probe_missing().await;
        assert!(missing.error.is_some_and(|e| e.starts_with("cannot start")));
    }

    async fn probe_missing() -> Probe {
        probe(&Config { claude_bin: "/nonexistent/claude".into(), ..Config::default() }, Path::new("/")).await
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate("héllo", 5), "héllo");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
        assert_eq!(truncate("", 3), "");
    }

    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        fn new(name: &str, body: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!("claude-notch-brain-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let script = dir.join("claude");
            let body = body.replace("$DIR", dir.to_str().unwrap());
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self { dir }
        }

        fn cfg(&self) -> Config {
            Config { claude_bin: self.dir.join("claude").to_string_lossy().into(), ..Config::default() }
        }

        async fn run(&self, cfg: &Config, cancel: oneshot::Receiver<()>) -> Vec<BrainEvent> {
            let mut events = Vec::new();
            run(cfg, "open firefox --help", None, &self.dir, Path::new("/bin/true"), cancel, |e| events.push(e)).await;
            events
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn runs_claude_with_prompt_on_stdin() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/text.jsonl");
        let fake =
            Fake::new("ok", &format!("cat > \"$DIR/stdin\"\nprintf '%s\\n' \"$@\" > \"$DIR/args\"\ncat {fixture}"));
        let (_keep, cancel) = oneshot::channel();
        let events = fake.run(&fake.cfg(), cancel).await;
        assert!(matches!(events[events.len() - 2], BrainEvent::Finished { is_error: false, .. }));
        assert_eq!(events.last(), Some(&BrainEvent::Exited { error: None }));
        assert_eq!(std::fs::read_to_string(fake.dir.join("stdin")).unwrap(), "open firefox --help");
        let argv = std::fs::read_to_string(fake.dir.join("args")).unwrap();
        assert!(argv.starts_with("-p\n"));
        assert!(!argv.contains("open firefox"), "prompt leaked into argv");
    }

    #[tokio::test]
    async fn exit_without_answer_reports_stderr() {
        let fake = Fake::new("fail", "cat >/dev/null\necho 'Invalid API key' >&2\nexit 3");
        let (_keep, cancel) = oneshot::channel();
        let events = fake.run(&fake.cfg(), cancel).await;
        assert_eq!(events, vec![BrainEvent::Exited { error: Some("Invalid API key".into()) }]);
    }

    #[tokio::test]
    async fn missing_binary_is_an_error() {
        let cfg = Config { claude_bin: "/nonexistent/claude".into(), ..Config::default() };
        let mut events = Vec::new();
        let (_keep, cancel) = oneshot::channel();
        run(&cfg, "x", None, Path::new("/"), Path::new("x"), cancel, |e| events.push(e)).await;
        assert!(matches!(&events[..], [BrainEvent::Exited { error: Some(e) }] if e.starts_with("cannot start")));
    }

    #[tokio::test]
    async fn cancel_kills_the_whole_process_group() {
        let fake = Fake::new("cancel", "cat >/dev/null\nsleep 30 &\necho $! > \"$DIR/child\"\nwait");
        let (tx, cancel) = oneshot::channel();
        let started = std::time::Instant::now();
        let cfg = fake.cfg();
        let (events, ()) = tokio::join!(fake.run(&cfg, cancel), async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            tx.send(()).unwrap();
        });
        assert_eq!(events, vec![BrainEvent::Exited { error: None }]);
        assert!(started.elapsed() < Duration::from_secs(5));
        let proc = PathBuf::from(format!("/proc/{}", std::fs::read_to_string(fake.dir.join("child")).unwrap().trim()));
        // The orphaned child is reaped by init shortly after it dies.
        for _ in 0..40 {
            if !proc.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!proc.exists(), "grandchild survived");
    }

    #[tokio::test]
    async fn timeout_stops_claude() {
        let fake = Fake::new("slow", "cat >/dev/null\nexec sleep 30");
        let cfg = Config { turn_timeout_secs: 1, ..fake.cfg() };
        let (_keep, cancel) = oneshot::channel();
        let events = fake.run(&cfg, cancel).await;
        assert!(matches!(&events[..], [BrainEvent::Exited { error: Some(e) }] if e.contains("longer than 1 s")));
    }
}
