//! The notch's own settings in `~/.config/claude-notch/config.toml`: only what Claude Code has no
//! setting for (wake phrases, speech recognition, the microphone, the notch itself). How Claude
//! behaves lives in Claude Code's own settings files; see `claude_settings.rs`.
//! Every setting is optional; a missing file means the defaults.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// Centre of the notch, as a fraction of the screen width.
    pub x: f64,
    /// Distance from the top of the screen, in pixels; 0 sits in the top edge like a notch.
    pub y: f64,
}

impl Default for Position {
    fn default() -> Self {
        Self { x: 0.5, y: 0.0 }
    }
}

impl Position {
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.x) || !(0.0..=4000.0).contains(&self.y) {
            return Err("position must have x between 0 and 1 and y between 0 and 4000".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Phrases that wake the notch when said at the start of an utterance.
    pub wake_phrases: Vec<String>,
    /// Whisper language code ("en", "de", …), or "auto": per utterance, the most likely of
    /// `auto_languages`.
    pub language: String,
    pub auto_languages: Vec<String>,
    /// ggml Whisper model; defaults to `<data dir>/models/ggml-small-q5_1.bin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_path: Option<PathBuf>,
    /// The Claude Code executable.
    pub claude_bin: String,
    /// After an answer, how long the next sentence counts as a command without the wake phrase. 0 turns it off.
    pub follow_up_secs: u64,
    /// A Claude run is stopped after this long.
    pub turn_timeout_secs: u64,
    pub sounds: bool,
    /// Read answers aloud with Kokoro (`scripts/download-model.sh kokoro`).
    pub speak: bool,
    /// Kokoro voice, a file in `<data dir>/models/kokoro/voices/`: `af_heart`, `am_michael`, …
    pub voice: String,
    /// Lowest microphone RMS (0–1) that can count as speech.
    pub vad_threshold: f32,
    pub position: Position,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wake_phrases: ["hi claude", "hey claude", "hello claude", "ok claude", "okay claude"]
                .map(String::from)
                .to_vec(),
            language: "en".into(),
            auto_languages: vec!["en".into()],
            model_path: None,
            claude_bin: "claude".into(),
            follow_up_secs: 60,
            turn_timeout_secs: 300,
            sounds: true,
            speak: false,
            voice: "af_heart".into(),
            vad_threshold: 0.012,
            position: Position::default(),
        }
    }
}

const HEADER: &str = "# Claude Notch settings, written by its Settings window. Editing by hand is fine,\n\
                      # but comments are not kept. Claude's model, effort and permissions live in\n\
                      # Claude Code's own settings files.\n\n";

impl Config {
    /// Reads the config file; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let cfg: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.wake_phrases.is_empty() {
            return Err("wake_phrases must not be empty".into());
        }
        for phrase in &self.wake_phrases {
            let words = crate::wake::normalize(phrase);
            if words.is_empty() || words.len() > 4 {
                return Err(format!("wake phrase {phrase:?} must have 1 to 4 words"));
            }
        }
        if self.language != "auto" && !is_language(&self.language) {
            return Err(format!(
                "language {:?} must be \"auto\" or a Whisper language code like \"en\"",
                self.language
            ));
        }
        if self.auto_languages.is_empty() || self.auto_languages.len() > 10 {
            return Err("auto_languages must list 1 to 10 languages".into());
        }
        if let Some(bad) = self.auto_languages.iter().find(|l| !is_language(l)) {
            return Err(format!("auto_languages: {bad:?} is not a Whisper language code"));
        }
        if self.claude_bin.trim().is_empty() {
            return Err("claude_bin must not be empty".into());
        }
        if self.follow_up_secs > 600 {
            return Err("follow_up_secs must be at most 600".into());
        }
        if !(10..=3600).contains(&self.turn_timeout_secs) {
            return Err("turn_timeout_secs must be between 10 and 3600".into());
        }
        if !crate::tts::valid_voice(&self.voice) {
            return Err(format!("voice {:?} must be a Kokoro voice name like \"af_heart\"", self.voice));
        }
        if !(0.001..=0.5).contains(&self.vad_threshold) {
            return Err("vad_threshold must be between 0.001 and 0.5".into());
        }
        self.position.validate()
    }

    /// Validates, then replaces the file atomically (readable by this user only).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let body = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        write_atomic(path, format!("{HEADER}{body}").as_bytes(), 0o600)
    }

    /// The languages speech recognition may choose from.
    pub fn speech_languages(&self) -> Vec<String> {
        if self.language == "auto" { self.auto_languages.clone() } else { vec![self.language.clone()] }
    }

    /// Settings read once at startup (speech recognition, microphone, wake phrases): a change
    /// takes effect after a restart. Everything else applies at once.
    pub fn needs_restart(&self, running: &Self) -> bool {
        self.wake_phrases != running.wake_phrases
            || self.speech_languages() != running.speech_languages()
            || self.model_path() != running.model_path()
            || self.claude_bin != running.claude_bin
            || self.vad_threshold.to_bits() != running.vad_threshold.to_bits()
    }

    pub fn model_path(&self) -> PathBuf {
        self.model_path.clone().unwrap_or_else(|| models_dir().join("ggml-small-q5_1.bin"))
    }
}

/// A language code Whisper knows ("en", "de", "fr", …).
fn is_language(code: &str) -> bool {
    // Checked before asking Whisper, whose lookup cannot take arbitrary bytes.
    (2..=5).contains(&code.len())
        && code.bytes().all(|b| b.is_ascii_lowercase())
        && whisper_rs::get_lang_id(code).is_some()
}

/// Replaces `path` with `bytes` so that a reader sees either the old or the new file, never half
/// of one: write a temporary file next to it, flush it to disk, rename it over.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.claude-notch-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| format!("{}: {e}", path.display()))
}

pub fn create_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// An XDG base directory, or its fallback under `$HOME` when unset or relative.
fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn config_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("claude-notch").join("config.toml")
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("claude-notch")
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Claude's working directory for voice commands. Its CLAUDE.md is the personality and
/// preferences; its `.claude/settings.json` holds Claude Code settings for voice only.
pub fn workspace_dir() -> PathBuf {
    data_dir().join("workspace")
}

/// `$XDG_RUNTIME_DIR/claude-notch.sock`. No fallback to /tmp: that directory is shared with other users.
pub fn socket_path() -> Result<PathBuf, String> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("claude-notch.sock"))
        .ok_or_else(|| "XDG_RUNTIME_DIR is not set".into())
}

const WORKSPACE_TEMPLATE: &str = include_str!("../assets/workspace-CLAUDE.md");

/// Writes `bytes` to a new file; an existing file is the user's and is left alone.
fn create_once(file: &Path, bytes: &[u8]) -> Result<(), String> {
    match std::fs::OpenOptions::new().write(true).create_new(true).open(file) {
        Ok(mut f) => f.write_all(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
    .map_err(|e| format!("{}: {e}", file.display()))
}

/// Creates the private workspace, its CLAUDE.md and its voice-only Claude Code settings once;
/// never overwrites what is there.
pub fn ensure_workspace(dir: &Path) -> Result<(), String> {
    create_private_dir(&dir.join(".claude"))?;
    create_once(&dir.join("CLAUDE.md"), WORKSPACE_TEMPLATE.as_bytes())?;
    create_once(&crate::claude_settings::voice_path(dir), crate::claude_settings::VOICE_DEFAULTS.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-notch-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn defaults_are_valid() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn parses_partial_file() {
        let cfg = Config::parse(
            "language = \"de\"\nfollow_up_secs = 0\nsounds = false\nspeak = true\nvoice = \"am_michael\"\n[position]\nx = 0.2\ny = 40.0\n",
        )
        .unwrap();
        assert!(cfg.speak);
        assert_eq!(cfg.voice, "am_michael");
        assert!(!Config::default().speak, "speaking needs a model download, so it is opt-in");
        assert_eq!(cfg.language, "de");
        assert_eq!(cfg.follow_up_secs, 0);
        assert!(!cfg.sounds);
        assert_eq!(cfg.position, Position { x: 0.2, y: 40.0 });
        assert_eq!(cfg.claude_bin, "claude");
        assert_eq!(cfg.speech_languages(), ["de"]);
        assert_eq!(Config::default().speech_languages(), ["en"]);
        let three = Config::parse("language = \"auto\"\nauto_languages = [\"en\", \"de\", \"fr\"]").unwrap();
        assert_eq!(three.speech_languages(), ["en", "de", "fr"]);
    }

    #[test]
    fn rejects_unknown_fields_and_bad_values() {
        for bad in [
            "wake_phrase = []",
            "wake_phrases = []",
            "wake_phrases = [\"...\"]",
            "wake_phrases = [\"one two three four five\"]",
            "language = \"English\"",
            "language = \"EN\"",
            "language = \"xx\"",
            "auto_languages = []",
            "auto_languages = [\"en\", \"klingon\"]",
            "claude_bin = \"  \"",
            // Claude's settings belong to Claude Code's files, not here.
            "claude_model = \"opus\"",
            "permission_mode = \"auto\"",
            "follow_up_secs = 601",
            "turn_timeout_secs = 5",
            "vad_threshold = 0.9",
            "voice = \"\"",
            "voice = \"../../etc/passwd\"",
            "voice = \"af_heart.bin\"",
            "vad_threshold = 0.0",
            "position = { x = 1.5, y = 0.0 }",
            "position = { x = 0.5, y = -1.0 }",
            "position = { x = 0.5, y = 0.0, z = 1.0 }",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn missing_file_gives_defaults() {
        let cfg = Config::load(Path::new("/nonexistent/claude-notch/config.toml")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn save_round_trips_privately_and_refuses_invalid() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("save");
        let path = dir.join("sub").join("config.toml");
        let cfg = Config {
            language: "de".into(),
            model_path: Some("/m/ggml.bin".into()),
            position: Position { x: 0.25, y: 12.0 },
            ..Config::default()
        };
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("# Claude Notch settings"));
        let bad = Config { follow_up_secs: 9999, ..cfg.clone() };
        assert!(bad.save(&path).is_err());
        assert_eq!(Config::load(&path).unwrap(), cfg, "a refused save leaves the file alone");
        let leftovers = std::fs::read_dir(dir.join("sub")).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary files left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restart_only_for_settings_read_at_startup() {
        let running = Config::default();
        for live in [
            Config { follow_up_secs: 10, ..running.clone() },
            Config { sounds: false, ..running.clone() },
            Config { speak: true, ..running.clone() },
            Config { voice: "am_michael".into(), ..running.clone() },
            Config { turn_timeout_secs: 60, ..running.clone() },
            Config { position: Position { x: 0.1, y: 5.0 }, ..running.clone() },
        ] {
            assert!(!live.needs_restart(&running), "{live:?}");
        }
        for restart in [
            Config { language: "de".into(), ..running.clone() },
            Config { language: "auto".into(), auto_languages: vec!["en".into(), "de".into()], ..running.clone() },
            Config { wake_phrases: vec!["yo claude".into()], ..running.clone() },
            Config { vad_threshold: 0.05, ..running.clone() },
            Config { model_path: Some("/x.bin".into()), ..running.clone() },
        ] {
            assert!(restart.needs_restart(&running), "{restart:?}");
        }
    }

    #[test]
    fn workspace_is_created_once_and_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("ws");
        ensure_workspace(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let voice = crate::claude_settings::voice_path(&dir);
        assert_eq!(crate::claude_settings::read(&voice).unwrap()["effortLevel"], "low");
        std::fs::write(dir.join("CLAUDE.md"), "mine").unwrap();
        std::fs::write(&voice, "{\"model\":\"haiku\"}").unwrap();
        ensure_workspace(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(&voice).unwrap(), "{\"model\":\"haiku\"}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
