//! Voice command history, read from Claude Code's own session transcripts: the notch keeps no
//! history of its own. Claude Code stores a project's sessions in
//! `~/.claude/projects/<working directory with every non-alphanumeric character as '-'>/<id>.jsonl`.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;

use crate::brain::{is_session_id as is_uuid, truncate};

const MAX_SESSIONS: usize = 50;
/// Larger transcripts are listed without reading them.
const MAX_READ: u64 = 20 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub id: String,
    /// Seconds since the epoch of the last change.
    pub updated: u64,
    pub first_prompt: String,
    pub prompts: usize,
    pub last_answer: String,
}

/// Where Claude Code keeps the sessions of commands run in `workspace`.
pub fn sessions_dir(workspace: &Path) -> PathBuf {
    let encoded: String =
        workspace.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::config::home().join(".claude"))
        .join("projects")
        .join(encoded)
}

/// The user's typed or spoken prompts and Claude's last text in one transcript.
fn summarize(lines: impl Iterator<Item = String>) -> (String, usize, String) {
    let (mut first, mut prompts, mut last) = (String::new(), 0, String::new());
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("user") => {
                // Tool results are user messages too, but with a list as content.
                if let Some(text) = v["message"]["content"].as_str().filter(|t| !t.starts_with('<')) {
                    prompts += 1;
                    if first.is_empty() {
                        first = truncate(text.trim(), 200);
                    }
                }
            }
            Some("assistant") => {
                let texts = v["message"]["content"].as_array().into_iter().flatten();
                if let Some(text) = texts.filter_map(|b| b["text"].as_str()).map(str::trim).rfind(|t| !t.is_empty()) {
                    last = truncate(text, 300);
                }
            }
            _ => {}
        }
    }
    (first, prompts, last)
}

/// The most recent voice sessions first.
pub fn list(dir: &Path) -> Vec<Session> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(u64, PathBuf, String, u64)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let id = path.file_stem()?.to_str()?.to_string();
            let meta = e.metadata().ok()?;
            let is_session = path.extension()? == "jsonl" && is_uuid(&id) && meta.is_file();
            let updated = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            is_session.then_some((updated, path, id, meta.len()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found
        .into_iter()
        .take(MAX_SESSIONS)
        .map(|(updated, path, id, size)| {
            let (first_prompt, prompts, last_answer) = match std::fs::File::open(&path) {
                Ok(file) if size <= MAX_READ => summarize(std::io::BufReader::new(file).lines().map_while(Result::ok)),
                _ => (String::new(), 0, String::new()),
            };
            Session { id, updated, first_prompt, prompts, last_answer }
        })
        .filter(|s| s.prompts > 0)
        .collect()
}

/// Deletes the voice sessions: `<uuid>.jsonl` files and `<uuid>` folders in `dir`, nothing else
/// (Claude Code keeps other things there, such as its memory). Returns how many sessions went.
pub fn delete_all(dir: &Path) -> Result<usize, String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(0) };
    let mut deleted = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // symlink_metadata: a link is never followed out of this folder.
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        let result = if meta.is_file() && name.strip_suffix(".jsonl").is_some_and(is_uuid) {
            deleted += 1;
            std::fs::remove_file(&path)
        } else if meta.is_dir() && is_uuid(&name) {
            std::fs::remove_dir_all(&path)
        } else {
            continue;
        };
        result.map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(deleted)
}

/// argv to open `claude --resume <id>` in a new window of `terminal`, started in `dir`.
pub fn terminal_command(terminal: &str, dir: &Path, id: &str) -> Option<Vec<String>> {
    let dir = dir.to_string_lossy().into_owned();
    let claude = ["claude", "--resume", id].map(String::from);
    let mut argv: Vec<String> = match terminal {
        "konsole" => vec!["konsole".into(), "--workdir".into(), dir, "-e".into()],
        "gnome-terminal" => vec!["gnome-terminal".into(), format!("--working-directory={dir}"), "--".into()],
        "kitty" => vec!["kitty".into(), "--directory".into(), dir],
        "alacritty" => vec!["alacritty".into(), "--working-directory".into(), dir, "-e".into()],
        "foot" => vec!["foot".into(), format!("--working-directory={dir}")],
        "xterm" => vec!["xterm".into(), "-e".into()],
        _ => return None,
    };
    argv.extend(claude);
    Some(argv)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join(program).is_file()))
}

/// Continues a voice session interactively in a new terminal window.
pub fn resume_in_terminal(id: &str, workspace: &Path) -> Result<(), String> {
    if !is_uuid(id) {
        return Err("not a session id".into());
    }
    let terminal = ["konsole", "gnome-terminal", "kitty", "alacritty", "foot", "xterm"]
        .into_iter()
        .find(|t| on_path(t))
        .ok_or("no supported terminal found (konsole, gnome-terminal, kitty, alacritty, foot, xterm)")?;
    let argv = terminal_command(terminal, workspace, id).ok_or("unsupported terminal")?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(workspace)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Its own process group, so stopping the notch never takes the terminal with it.
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().map(drop).map_err(|e| format!("cannot start {terminal}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4700cef8-84e7-4458-a427-1d252586e54c";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-notch-hist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn transcript(prompts: &[&str], answer: &str) -> String {
        let mut lines = vec![r#"{"type":"queue-operation","operation":"enqueue"}"#.to_string()];
        for p in prompts {
            lines.push(serde_json::json!({"type":"user","message":{"role":"user","content":p}}).to_string());
            lines.push(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#.into());
            lines.push(
                serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":answer}]}})
                    .to_string(),
            );
        }
        lines.push("not json".into());
        lines.join("\n")
    }

    #[test]
    fn project_folder_name_matches_claude_code() {
        let dir = sessions_dir(Path::new("/home/omidiyanto/.local/share/claude-notch/workspace"));
        assert!(dir.ends_with("projects/-home-omidiyanto--local-share-claude-notch-workspace"), "{dir:?}");
    }

    #[test]
    fn lists_sessions_with_their_prompts_and_last_answer() {
        let dir = temp("list");
        std::fs::write(
            dir.join(format!("{ID}.jsonl")),
            transcript(&["Can you open YouTube?", "turn it down"], "Sure."),
        )
        .unwrap();
        std::fs::write(dir.join("not-a-uuid.jsonl"), transcript(&["x"], "y")).unwrap();
        std::fs::write(dir.join("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl"), "{}").unwrap();
        let sessions = list(&dir);
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        let s = &sessions[0];
        assert_eq!(
            (s.id.as_str(), s.first_prompt.as_str(), s.prompts, s.last_answer.as_str()),
            (ID, "Can you open YouTube?", 2, "Sure.")
        );
        assert!(s.updated > 0);
        assert_eq!(list(&dir.join("missing")), Vec::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_only_removes_sessions() {
        let dir = temp("delete");
        std::fs::write(dir.join(format!("{ID}.jsonl")), "{}").unwrap();
        std::fs::create_dir(dir.join(ID)).unwrap();
        std::fs::write(dir.join(ID).join("tool-result.txt"), "x").unwrap();
        std::fs::create_dir(dir.join("memory")).unwrap();
        std::fs::write(dir.join("memory").join("MEMORY.md"), "keep").unwrap();
        std::fs::write(dir.join("notes.jsonl"), "keep").unwrap();
        let outside = temp("outside");
        std::os::unix::fs::symlink(&outside, dir.join("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")).unwrap();

        assert_eq!(delete_all(&dir).unwrap(), 1);
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", "memory", "notes.jsonl"]);
        assert!(outside.exists(), "a symlink is never followed");
        assert_eq!(delete_all(&dir.join("missing")).unwrap(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn terminal_commands() {
        let dir = Path::new("/w s");
        let resume = ["claude", "--resume", ID];
        assert_eq!(
            terminal_command("konsole", dir, ID).unwrap()[..5],
            ["konsole", "--workdir", "/w s", "-e", "claude"]
        );
        assert_eq!(terminal_command("gnome-terminal", dir, ID).unwrap()[1..3], ["--working-directory=/w s", "--"]);
        for t in ["konsole", "gnome-terminal", "kitty", "alacritty", "foot", "xterm"] {
            assert!(terminal_command(t, dir, ID).unwrap().ends_with(&resume.map(String::from)), "{t}");
        }
        assert_eq!(terminal_command("sh", dir, ID), None);
    }

    #[test]
    fn resume_refuses_anything_but_a_session_id() {
        assert!(resume_in_terminal("--help; rm -rf ~", Path::new("/tmp")).is_err());
    }
}
