//! Claude Code's own settings files are the single source of truth for how Claude behaves on a
//! voice command: model, effort, permission mode and permission rules. Two scopes matter:
//!   * user: `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`), every Claude Code session;
//!   * voice: `<workspace>/.claude/settings.json`, Claude Code's project scope for the voice
//!     workspace, which overrides the user scope for voice commands only.
//!
//! The notch reads both. It writes one only when the user saves the Claude tab of its settings
//! window, and then only the keys below whose value actually changed, merged into the file as it
//! is on disk at that moment, after a backup, atomically. Every other key keeps its value and its
//! place. A file that is not valid JSON is never touched.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Created once in a new workspace. Effort low: voice commands are short desktop tasks. The
/// allowlist only holds commands that cannot hurt; never a generic launcher (`setsid`,
/// `konsole -e`, `bash -c`), which would run anything without review.
pub const VOICE_DEFAULTS: &str = r#"{
  "$schema": "https://json.schemastore.org/claude-code-settings.json",
  "effortLevel": "low",
  "permissions": {
    "allow": [
      "Bash(xdg-open:*)",
      "Bash(setsid -f firefox:*)",
      "Bash(firefox:*)",
      "Bash(playerctl:*)",
      "Bash(pactl set-sink-volume:*)",
      "Bash(pactl set-sink-mute:*)",
      "Bash(notify-send:*)",
      "WebSearch"
    ]
  }
}
"#;

const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];
const MODES: &[&str] = &["default", "acceptEdits", "plan", "dontAsk", "auto"];
const KEEP_BACKUPS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Voice,
    User,
}

/// The keys the notch manages. `None` means the file does not set it (inherited).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Managed {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub allow: Option<Vec<String>>,
}

pub fn user_path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::config::home().join(".claude"))
        .join("settings.json")
}

pub fn voice_path(workspace: &Path) -> PathBuf {
    workspace.join(".claude").join("settings.json")
}

pub fn path(scope: Scope, workspace: &Path) -> PathBuf {
    match scope {
        Scope::Voice => voice_path(workspace),
        Scope::User => user_path(),
    }
}

fn parse(text: &str) -> Result<Value, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e}); fix it in Claude Code first"))?;
    if value.is_object() { Ok(value) } else { Err("not a JSON object; fix it in Claude Code first".into()) }
}

/// The file as JSON; a missing file is an empty object.
pub fn read(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn extract(v: &Value) -> Managed {
    let text = |pointer: &str| v.pointer(pointer).and_then(Value::as_str).map(String::from);
    Managed {
        model: text("/model"),
        effort: text("/effortLevel"),
        mode: text("/permissions/defaultMode"),
        allow: v
            .pointer("/permissions/allow")
            .and_then(Value::as_array)
            .map(|rules| rules.iter().filter_map(|r| r.as_str().map(String::from)).collect()),
    }
}

fn set(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    match value {
        // An existing key keeps its place; a new one goes last.
        Some(value) => {
            map.insert(key.into(), value);
        }
        None => {
            map.shift_remove(key);
        }
    }
}

/// Writes `new` into the settings object, touching only the keys whose value differs from
/// what the object already holds.
pub fn apply(v: &mut Value, new: &Managed) -> Result<(), String> {
    let old = extract(v);
    let root = v.as_object_mut().ok_or("settings are not a JSON object")?;
    if new.model != old.model {
        set(root, "model", new.model.clone().map(Value::from));
    }
    if new.effort != old.effort {
        set(root, "effortLevel", new.effort.clone().map(Value::from));
    }
    if new.mode == old.mode && new.allow == old.allow {
        return Ok(());
    }
    let permissions = root.entry("permissions").or_insert_with(|| json!({}));
    let permissions =
        permissions.as_object_mut().ok_or("\"permissions\" is not an object; fix it in Claude Code first")?;
    if new.mode != old.mode {
        set(permissions, "defaultMode", new.mode.clone().map(Value::from));
    }
    if new.allow != old.allow {
        set(permissions, "allow", new.allow.clone().map(Value::from));
    }
    if permissions.is_empty() {
        root.shift_remove("permissions");
    }
    Ok(())
}

pub fn validate(m: &Managed, scope: Scope) -> Result<(), String> {
    if let Some(model) = &m.model
        && (model.is_empty()
            || model.len() > 80
            || !model.chars().all(|c| c.is_ascii_alphanumeric() || "-._[]:/".contains(c)))
    {
        return Err(format!("model {model:?} is not a model alias or id"));
    }
    if let Some(effort) = &m.effort
        && !EFFORTS.contains(&effort.as_str())
    {
        return Err("effort must be low, medium, high or xhigh".into());
    }
    match m.mode.as_deref() {
        // A voice command can be started by a TV or someone else in the room.
        Some("bypassPermissions") => return Err("Claude Notch never sets bypassPermissions".into()),
        Some("auto") if scope == Scope::Voice => {
            return Err("Claude Code only takes auto mode from your user settings: pick \"All sessions\"".into());
        }
        Some(mode) if !MODES.contains(&mode) => return Err(format!("unknown permission mode {mode:?}")),
        _ => {}
    }
    if let Some(rules) = &m.allow {
        if rules.len() > 200 {
            return Err("at most 200 permission rules".into());
        }
        if let Some(bad) =
            rules.iter().find(|r| r.trim().is_empty() || r.len() > 300 || r.chars().any(char::is_control))
        {
            return Err(format!("permission rule {bad:?} is invalid"));
        }
    }
    Ok(())
}

/// Keeps the original next to the notch's data, never inside Claude Code's directories.
fn backup(dir: &Path, scope: Scope, text: &str) -> Result<(), String> {
    crate::config::create_private_dir(dir)?;
    let prefix = format!("{}-settings-", if scope == Scope::User { "user" } else { "voice" });
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let name = format!("{prefix}{}-{:09}.json", stamp.as_secs(), stamp.subsec_nanos());
    crate::config::write_atomic(&dir.join(name), text.as_bytes(), 0o600)?;
    let mut old: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
        .collect();
    old.sort();
    for path in old.iter().take(old.len().saturating_sub(KEEP_BACKUPS)) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// Saves `new` into the file at `path`. Returns whether the file changed.
pub fn save(path: &Path, scope: Scope, new: &Managed, backups: &Path) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    validate(new, scope)?;
    for _ in 0..3 {
        let original = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut settings = match &original {
            Some(text) => parse(text).map_err(|e| format!("{}: {e}", path.display()))?,
            None => json!({}),
        };
        let before = settings.clone();
        apply(&mut settings, new)?;
        if settings == before {
            return Ok(false);
        }
        let mut text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        text.push('\n');
        if let Some(original) = &original {
            backup(backups, scope, original)?;
        }
        // Claude Code writes this file too (/config, /model): if it did since we read it,
        // start over from its version instead of overwriting it.
        if std::fs::read_to_string(path).ok() != original {
            continue;
        }
        let mode = std::fs::metadata(path).map_or(0o600, |m| m.permissions().mode() & 0o777);
        if let Some(dir) = path.parent() {
            crate::config::create_private_dir(dir)?;
        }
        crate::config::write_atomic(path, text.as_bytes(), mode)?;
        return Ok(true);
    }
    Err(format!("{} kept changing while saving; try again", path.display()))
}

/// The permission mode a voice command starts in, as far as these two files tell.
pub fn effective_mode(user: &Value, voice: &Value) -> Option<String> {
    let mode = |v: &Value| v.pointer("/permissions/defaultMode").and_then(Value::as_str).map(String::from);
    // Claude Code ignores auto and bypassPermissions in project files.
    mode(voice).filter(|m| m != "auto" && m != "bypassPermissions").or_else(|| mode(user))
}

/// The `--permission-mode` to force on a voice command, if any: a user-wide bypassPermissions is
/// lowered to auto, because anyone in the room can start a voice command.
pub fn forced_mode(workspace: &Path) -> Option<&'static str> {
    let user = read(&user_path()).unwrap_or_else(|_| json!({}));
    let voice = read(&voice_path(workspace)).unwrap_or_else(|_| json!({}));
    (effective_mode(&user, &voice).as_deref() == Some("bypassPermissions")).then_some("auto")
}

/// Claude Code's own global state file (sign-in, MCP servers, `/chrome` choices). Only read.
pub fn global_path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(crate::config::home)
        .join(".claude.json")
}

/// Whether the user turned on Claude in Chrome by default with `/chrome`. Claude Code applies
/// that choice only to interactive sessions; a `claude -p` run needs `--chrome` to get the
/// browser tools (measured on Claude Code 2.1.302: 0 browser tools without it, 22 with it).
pub fn chrome_by_default(global: &Value) -> bool {
    global["claudeInChromeDefaultEnabled"].as_bool() == Some(true)
}

pub fn chrome_enabled() -> bool {
    read(&global_path()).is_ok_and(|v| chrome_by_default(&v))
}

#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    pub path: String,
    pub settings: Managed,
    /// Why the file cannot be read or edited (invalid JSON, …).
    pub error: Option<String>,
}

pub fn view(scope: Scope, workspace: &Path) -> FileView {
    let path = path(scope, workspace);
    let (settings, error) = match read(&path) {
        Ok(v) => (extract(&v), None),
        Err(e) => (Managed::default(), Some(e)),
    };
    FileView { path: path.display().to_string(), settings, error }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a real ~/.claude/settings.json as Claude Code writes it.
    const USER: &str = r#"{
  "permissions": {
    "defaultMode": "auto",
    "deny": [
      "Read(./.env)"
    ]
  },
  "model": "opus",
  "enabledPlugins": {
    "ponytail@ponytail": true
  },
  "modelSettings": {
    "claude-opus-5-5": {
      "effortLevel": "high"
    }
  },
  "theme": "dark",
  "language": "english"
}
"#;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-notch-cs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pretty(v: &Value) -> String {
        serde_json::to_string_pretty(v).unwrap() + "\n"
    }

    #[test]
    fn voice_defaults_are_valid_and_safe() {
        let v = parse(VOICE_DEFAULTS).unwrap();
        let m = extract(&v);
        assert_eq!(m.effort.as_deref(), Some("low"));
        assert_eq!(m.mode, None, "the permission mode is inherited from the user");
        validate(&m, Scope::Voice).unwrap();
        for rule in m.allow.unwrap() {
            for risky in ["Bash(setsid:", "Bash(konsole", "Bash(bash", "Bash(sh ", "Bash(*", "Bash)"] {
                assert!(!rule.starts_with(risky), "{rule} would run anything");
            }
            assert_ne!(rule, "Bash");
        }
    }

    #[test]
    fn extracts_only_the_managed_keys() {
        let m = extract(&parse(USER).unwrap());
        assert_eq!(m, Managed { model: Some("opus".into()), effort: None, mode: Some("auto".into()), allow: None });
    }

    #[test]
    fn unchanged_settings_are_written_back_byte_for_byte() {
        let mut v = parse(USER).unwrap();
        let same = extract(&v);
        apply(&mut v, &same).unwrap();
        assert_eq!(pretty(&v), USER);
    }

    #[test]
    fn a_change_touches_only_its_own_line() {
        let mut v = parse(USER).unwrap();
        let new = Managed { model: Some("sonnet".into()), ..extract(&v) };
        apply(&mut v, &new).unwrap();
        let after = pretty(&v);
        let changed: Vec<_> = USER.lines().zip(after.lines()).filter(|(a, b)| a != b).collect();
        assert_eq!(changed, vec![(r#"  "model": "opus","#, r#"  "model": "sonnet","#)]);
    }

    #[test]
    fn adding_and_removing_keys_keeps_the_rest() {
        let mut v = parse(USER).unwrap();
        let new =
            Managed { effort: Some("low".into()), mode: None, allow: Some(vec!["WebSearch".into()]), ..extract(&v) };
        apply(&mut v, &new).unwrap();
        assert_eq!(v["effortLevel"], "low");
        assert_eq!(v["permissions"], json!({ "deny": ["Read(./.env)"], "allow": ["WebSearch"] }));
        assert_eq!(v["modelSettings"]["claude-opus-5-5"]["effortLevel"], "high");
        assert_eq!(v["enabledPlugins"]["ponytail@ponytail"], true);

        // Emptied by us: the whole "permissions" object goes, nothing else.
        let mut only_mode = json!({ "permissions": { "defaultMode": "default" }, "theme": "dark" });
        apply(&mut only_mode, &Managed::default()).unwrap();
        assert_eq!(only_mode, json!({ "theme": "dark" }));
    }

    #[test]
    fn a_malformed_permissions_value_is_left_alone() {
        let mut v = json!({ "permissions": "oops" });
        assert!(apply(&mut v, &Managed { mode: Some("default".into()), ..Managed::default() }).is_err());
        assert_eq!(v, json!({ "permissions": "oops" }));
    }

    #[test]
    fn validation() {
        let ok = Managed {
            model: Some("claude-opus-5-5[1m]".into()),
            effort: Some("xhigh".into()),
            mode: Some("default".into()),
            allow: None,
        };
        validate(&ok, Scope::Voice).unwrap();
        validate(&Managed { mode: Some("auto".into()), ..Managed::default() }, Scope::User).unwrap();
        for (bad, scope) in [
            (Managed { mode: Some("auto".into()), ..Managed::default() }, Scope::Voice),
            (Managed { mode: Some("bypassPermissions".into()), ..Managed::default() }, Scope::User),
            (Managed { mode: Some("yolo".into()), ..Managed::default() }, Scope::User),
            (Managed { effort: Some("max".into()), ..Managed::default() }, Scope::User),
            (Managed { model: Some("opus; rm -rf ~".into()), ..Managed::default() }, Scope::User),
            (Managed { model: Some(String::new()), ..Managed::default() }, Scope::User),
            (Managed { allow: Some(vec![String::new()]), ..Managed::default() }, Scope::Voice),
            (Managed { allow: Some(vec!["Read\nBash".into()]), ..Managed::default() }, Scope::Voice),
        ] {
            assert!(validate(&bad, scope).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn save_merges_backs_up_and_keeps_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("save");
        let (file, backups) = (dir.join("settings.json"), dir.join("backups"));
        std::fs::write(&file, USER).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

        let current = extract(&read(&file).unwrap());
        assert!(!save(&file, Scope::User, &current, &backups).unwrap(), "no change, no write");
        assert!(!backups.exists(), "no change, no backup");

        let new = Managed { effort: Some("medium".into()), ..current };
        assert!(save(&file, Scope::User, &new, &backups).unwrap());
        let saved = read(&file).unwrap();
        assert_eq!(saved["effortLevel"], "medium");
        assert_eq!(saved["theme"], "dark");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o644);
        let kept: Vec<_> = std::fs::read_dir(&backups).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), USER);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "no temporary files left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_creates_a_missing_file_privately() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("new");
        let file = dir.join(".claude").join("settings.json");
        assert!(
            save(&file, Scope::Voice, &Managed { effort: Some("low".into()), ..Managed::default() }, &dir).unwrap()
        );
        assert_eq!(read(&file).unwrap(), json!({ "effortLevel": "low" }));
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_file_is_never_touched() {
        let dir = temp("broken");
        let file = dir.join("settings.json");
        let broken = "{ \"model\": \"opus\", // a comment\n}";
        std::fs::write(&file, broken).unwrap();
        let err =
            save(&file, Scope::User, &Managed { model: Some("haiku".into()), ..Managed::default() }, &dir).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), broken);
        // The settings window shows why instead of editing it.
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(voice_path(&dir), broken).unwrap();
        let shown = view(Scope::Voice, &dir);
        assert!(shown.error.is_some_and(|e| e.contains("not valid JSON")));
        assert_eq!(shown.settings, Managed::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_values_are_refused_before_any_write() {
        let dir = temp("refuse");
        let file = dir.join("settings.json");
        std::fs::write(&file, USER).unwrap();
        let bypass = Managed { mode: Some("bypassPermissions".into()), ..Managed::default() };
        assert!(save(&file, Scope::User, &bypass, &dir.join("b")).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), USER);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backups_are_pruned() {
        let dir = temp("prune");
        for i in 0..KEEP_BACKUPS + 4 {
            backup(&dir, Scope::Voice, &i.to_string()).unwrap();
        }
        backup(&dir, Scope::User, "user").unwrap();
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.iter().filter(|n| n.starts_with("voice-")).count(), KEEP_BACKUPS);
        assert_eq!(names.iter().filter(|n| n.starts_with("user-")).count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chrome_follows_the_users_own_choice() {
        assert!(chrome_by_default(&json!({ "claudeInChromeDefaultEnabled": true, "numStartups": 3 })));
        assert!(!chrome_by_default(&json!({ "claudeInChromeDefaultEnabled": false })));
        assert!(!chrome_by_default(&json!({ "claudeInChromeDefaultEnabled": "yes" })));
        assert!(!chrome_by_default(&json!({})));
    }

    #[test]
    fn effective_mode_follows_claude_codes_rules() {
        let user = |m: &str| json!({ "permissions": { "defaultMode": m } });
        let empty = json!({});
        assert_eq!(effective_mode(&user("auto"), &empty).as_deref(), Some("auto"));
        assert_eq!(effective_mode(&user("auto"), &user("default")).as_deref(), Some("default"));
        // Project files cannot turn on auto or bypassPermissions.
        assert_eq!(effective_mode(&user("default"), &user("auto")).as_deref(), Some("default"));
        assert_eq!(effective_mode(&user("bypassPermissions"), &empty).as_deref(), Some("bypassPermissions"));
        assert_eq!(effective_mode(&empty, &empty), None);
    }
}
