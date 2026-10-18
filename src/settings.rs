//! The settings window and everything it can do. Claude's behaviour is read from and written to
//! Claude Code's own settings files (`claude_settings.rs`); the notch's own settings go to
//! config.toml; personality is the workspace's CLAUDE.md; history is Claude Code's transcripts.
//! The page never sends a file path: every location is chosen here.
#![expect(clippy::needless_pass_by_value, reason = "Tauri passes command arguments by value")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, PoisonError};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::app::{Input, Shared};
use crate::brain::Probe;
use crate::claude_settings::{self, FileView, Managed, Scope};
use crate::config::{self, Config, Position};
use crate::history::{self, Session};

/// A CLAUDE.md larger than this is not something to edit in a text box.
const MAX_PERSONALITY: usize = 64 * 1024;

/// A section of the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    Claude,
    Personality,
    Voice,
    History,
    About,
}

impl Tab {
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Personality => "personality",
            Self::Voice => "voice",
            Self::History => "history",
            Self::About => "about",
        }
    }
}

pub fn open_window(app: &AppHandle, tab: Option<Tab>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.unminimize();
        if let Some(tab) = tab {
            // A fixed word from the enum above, never text from outside.
            let _ = window.eval(format!("location.hash = '{}'", tab.name()));
        }
        return window.set_focus().map_err(|e| e.to_string());
    }
    let page = tab.map_or_else(|| "settings.html".to_string(), |t| format!("settings.html#{}", t.name()));
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App(page.into()))
        .title("Claude Notch · Settings")
        .inner_size(860.0, 660.0)
        .min_inner_size(640.0, 480.0)
        .center()
        .build()
        .map(drop)
        .map_err(|e| e.to_string())
}

fn backups_dir() -> PathBuf {
    config::data_dir().join("backups")
}

fn current(shared: &Shared) -> Config {
    shared.config.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Changes the saved notch settings under one lock, writes them, and hands them to the app.
fn store(shared: &Shared, change: impl FnOnce(&mut Config)) -> Result<Config, String> {
    let mut saved = shared.config.lock().unwrap_or_else(PoisonError::into_inner);
    let mut next = saved.clone();
    change(&mut next);
    next.save(&config::config_file())?;
    *saved = next.clone();
    let _ = shared.tx.send(Input::Config(Arc::new(next.clone())));
    Ok(next)
}

#[derive(Debug, Serialize)]
pub struct Paths {
    config: String,
    workspace: String,
    personality: String,
    models: String,
    history: String,
    backups: String,
}

#[derive(Debug, Serialize)]
pub struct SettingsView {
    notch: Config,
    defaults: Config,
    restart_needed: bool,
    speech_models: Vec<String>,
    voice: FileView,
    user: FileView,
    /// Claude in Chrome turned on with `/chrome` → "Enabled by default".
    chrome: bool,
    paths: Paths,
    version: &'static str,
}

fn speech_models() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(config::models_dir()) else { return Vec::new() };
    let mut models: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "bin") && p.is_file())
        .map(|p| p.display().to_string())
        .collect();
    models.sort();
    models
}

#[tauri::command]
pub async fn open_settings(app: AppHandle) -> Result<(), String> {
    open_window(&app, None)
}

#[tauri::command]
pub fn get_settings(shared: tauri::State<'_, Shared>) -> SettingsView {
    let notch = current(&shared);
    let ws = &shared.workspace;
    SettingsView {
        restart_needed: notch.needs_restart(&shared.started),
        notch,
        defaults: Config::default(),
        speech_models: speech_models(),
        voice: claude_settings::view(Scope::Voice, ws),
        user: claude_settings::view(Scope::User, ws),
        chrome: claude_settings::chrome_enabled(),
        paths: Paths {
            config: config::config_file().display().to_string(),
            workspace: ws.display().to_string(),
            personality: ws.join("CLAUDE.md").display().to_string(),
            models: config::models_dir().display().to_string(),
            history: history::sessions_dir(ws).display().to_string(),
            backups: backups_dir().display().to_string(),
        },
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// Saves the notch's own settings; returns whether a restart is needed for all of them to apply.
#[tauri::command]
pub fn save_notch(shared: tauri::State<'_, Shared>, notch: Config) -> Result<bool, String> {
    // The position belongs to dragging the notch, not to this form.
    let saved = store(&shared, |c| *c = Config { position: c.position, ..notch })?;
    Ok(saved.needs_restart(&shared.started))
}

/// Merges Claude's settings into Claude Code's settings file for `scope`.
#[tauri::command]
pub fn save_claude(shared: tauri::State<'_, Shared>, scope: Scope, settings: Managed) -> Result<bool, String> {
    let path = claude_settings::path(scope, &shared.workspace);
    claude_settings::save(&path, scope, &settings, &backups_dir())
}

#[tauri::command]
pub fn read_personality(shared: tauri::State<'_, Shared>) -> Result<String, String> {
    let path = shared.workspace.join("CLAUDE.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[tauri::command]
pub fn save_personality(shared: tauri::State<'_, Shared>, text: String) -> Result<(), String> {
    if text.len() > MAX_PERSONALITY {
        return Err(format!("CLAUDE.md is limited to {} KiB here", MAX_PERSONALITY / 1024));
    }
    config::write_atomic(&shared.workspace.join("CLAUDE.md"), text.as_bytes(), 0o600)
}

#[tauri::command]
pub async fn test_claude(shared: tauri::State<'_, Shared>) -> Result<Probe, String> {
    let (cfg, workspace) = (current(&shared), shared.workspace.clone());
    Ok(crate::brain::probe(&cfg, &workspace).await)
}

#[tauri::command]
pub fn list_history(shared: tauri::State<'_, Shared>) -> Vec<Session> {
    history::list(&history::sessions_dir(&shared.workspace))
}

#[tauri::command]
pub fn delete_history(shared: tauri::State<'_, Shared>) -> Result<usize, String> {
    history::delete_all(&history::sessions_dir(&shared.workspace))
}

#[tauri::command]
pub fn resume_session(shared: tauri::State<'_, Shared>, id: String) -> Result<(), String> {
    history::resume_in_terminal(&id, &shared.workspace)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    Config,
    Workspace,
    Personality,
    VoiceSettings,
    UserSettings,
    Models,
    History,
    Backups,
}

fn location(which: Location, workspace: &Path) -> PathBuf {
    match which {
        Location::Config => config::config_file(),
        Location::Workspace => workspace.to_path_buf(),
        Location::Personality => workspace.join("CLAUDE.md"),
        Location::VoiceSettings => claude_settings::voice_path(workspace),
        Location::UserSettings => claude_settings::user_path(),
        Location::Models => config::models_dir(),
        Location::History => history::sessions_dir(workspace),
        Location::Backups => backups_dir(),
    }
}

/// Opens a file or folder in the desktop's default app; a missing file opens its folder.
#[tauri::command]
pub fn open_location(shared: tauri::State<'_, Shared>, which: Location) -> Result<(), String> {
    let mut path = location(which, &shared.workspace);
    while !path.exists() {
        path = path.parent().ok_or("nothing to open")?.to_path_buf();
    }
    let mut command = Command::new("xdg-open");
    command.arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().map(drop).map_err(|e| format!("xdg-open: {e}"))
}

#[tauri::command]
pub fn restart_notch(shared: tauri::State<'_, Shared>) -> Result<(), String> {
    shared.tx.send(Input::Restart).map_err(|_| "Claude Notch is shutting down".into())
}

#[tauri::command]
pub fn notch_position(shared: tauri::State<'_, Shared>) -> Position {
    current(&shared).position
}

#[tauri::command]
pub fn save_position(shared: tauri::State<'_, Shared>, x: f64, y: f64) -> Result<(), String> {
    let position = Position { x, y };
    position.validate()?;
    store(&shared, |c| c.position = position).map(drop)
}

#[tauri::command]
pub fn reset_position(app: AppHandle, shared: tauri::State<'_, Shared>) -> Result<(), String> {
    store(&shared, |c| c.position = Position::default())?;
    app.emit("position", Position::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_location_is_one_of_ours() {
        let ws = Path::new("/home/u/.local/share/claude-notch/workspace");
        for which in [
            Location::Config,
            Location::Workspace,
            Location::Personality,
            Location::VoiceSettings,
            Location::UserSettings,
            Location::Models,
            Location::History,
            Location::Backups,
        ] {
            let path = location(which, ws);
            let s = path.to_string_lossy();
            assert!(s.contains("claude-notch") || s.contains(".claude"), "{which:?}: {s}");
            assert!(path.is_absolute() || std::env::var_os("HOME").is_none(), "{which:?}: {s}");
        }
        assert!(serde_json::from_str::<Location>("\"/etc/passwd\"").is_err());
    }

    #[test]
    fn tab_names_match_the_page() {
        for tab in [Tab::Claude, Tab::Personality, Tab::Voice, Tab::History, Tab::About] {
            assert_eq!(serde_json::to_value(tab).unwrap(), tab.name());
        }
        assert!(serde_json::from_str::<Tab>("\"'; alert(1); '\"").is_err());
    }
}
