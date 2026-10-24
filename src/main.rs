//! claude-notch: a dynamic island for Linux desktops that hands your voice to Claude Code.

mod app;
mod approve;
mod audio;
mod brain;
mod claude_settings;
mod config;
mod history;
mod ipc;
mod island;
mod machine;
mod settings;
mod stt;
mod tts;
mod vad;
mod wake;

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use ipc::Request;

const USAGE: &str = "\
Usage: claude-notch [COMMAND]

Without a command, starts the notch.

Commands (sent to the running notch):
  listen        Start listening, as if you said \"hi Claude\" (bind it to a shortcut)
  cancel        Stop what Claude is doing
  pause         Turn the microphone off or back on
  settings [tab]  Open the settings window, at claude, personality, voice,
                history or about
  ask <text>    Give Claude a typed command, no wake phrase needed
  demo <mode>   Show one look: idle, sleep, listening, transcribing, thinking,
                working, approval, done, error

Internal:
  mcp-approve   Permission server started by Claude Code";

/// Set on a notch started by "Restart" to the process it replaces.
pub const REPLACES_VAR: &str = "CLAUDE_NOTCH_REPLACES";

pub fn log(message: &str) {
    eprintln!("[claude-notch] {message}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (request, arity) = match args.first().map(String::as_str) {
        None => {
            let replaces = std::env::var(REPLACES_VAR).ok().and_then(|pid| pid.parse().ok());
            // SAFETY: first thing in main, before any other thread exists. Not inherited by
            // the processes the notch starts.
            unsafe { std::env::remove_var(REPLACES_VAR) };
            island::prefer_x11_on_gnome();
            return app::run(replaces);
        }
        Some("mcp-approve") => return u8::try_from(approve::serve_stdio()).map_or(ExitCode::FAILURE, ExitCode::from),
        Some("-h" | "--help" | "help") => {
            let _ = writeln!(std::io::stdout(), "{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("-V" | "--version") => {
            let _ = writeln!(std::io::stdout(), "claude-notch {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("listen") => (Request::Listen, 1),
        Some("cancel") => (Request::Cancel, 1),
        Some("pause") => (Request::Pause, 1),
        Some("settings") => match args.get(1).map(|t| serde_json::from_value(serde_json::Value::from(t.as_str()))) {
            None => (Request::Settings { tab: None }, 1),
            Some(Ok(tab)) => (Request::Settings { tab: Some(tab) }, 2),
            Some(Err(_)) => (Request::Listen, 0),
        },
        Some("ask") if args.len() >= 2 => (Request::Ask { text: args[1..].join(" ") }, args.len()),
        Some("demo") if args.len() == 2 => match serde_json::from_value(serde_json::Value::from(args[1].as_str())) {
            Ok(mode) => (Request::Demo { mode }, 2),
            Err(_) => (Request::Listen, 0),
        },
        Some(_) => (Request::Listen, 0),
    };
    if args.len() != arity {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    match config::socket_path().and_then(|path| ipc::request(&path, &request, Duration::from_secs(5))) {
        Ok(reply) if reply.ok => ExitCode::SUCCESS,
        Ok(reply) => {
            log(&reply.error.unwrap_or_else(|| "refused".into()));
            ExitCode::FAILURE
        }
        Err(e) => {
            log(&e);
            ExitCode::FAILURE
        }
    }
}
