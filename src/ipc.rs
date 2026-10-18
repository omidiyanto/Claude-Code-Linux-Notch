//! A private Unix socket for everything that talks to the running notch: `claude-notch listen`
//! (bound to a desktop shortcut), the permission server Claude Code starts, and the second
//! instance that should exit. One JSON request and one JSON reply per connection.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::machine::Mode;

/// A Write tool's permission request carries the whole file, so leave room for that.
const MAX_MESSAGE: u64 = 4 << 20;
const MAX_ASK: usize = 4000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Listen,
    Cancel,
    Pause,
    /// Opens the settings window, at a section if given.
    Settings {
        #[serde(default)]
        tab: Option<crate::settings::Tab>,
    },
    /// A typed command, handled like a spoken one.
    Ask {
        text: String,
    },
    /// Shows one look of the notch, for trying the UI without a microphone.
    Demo {
        mode: Mode,
    },
    Approve {
        tool_name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a connection hands to the app. `Approve` waits on `reply` for the user's click.
#[derive(Debug)]
pub enum Incoming {
    Listen,
    Cancel,
    Pause,
    Settings(Option<crate::settings::Tab>),
    Ask(String),
    Demo(Mode),
    Approve { tool_name: String, input: Value, reply: oneshot::Sender<bool> },
}

#[derive(Debug)]
pub enum BindError {
    AlreadyRunning,
    Io(String),
}

/// Binds the socket, replacing a stale one left by a crash. Readable by this user only.
pub fn bind(path: &Path) -> Result<std::os::unix::net::UnixListener, BindError> {
    if UnixStream::connect(path).is_ok() {
        return Err(BindError::AlreadyRunning);
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(BindError::Io(format!("{}: {e}", path.display()))),
    }
    let listener =
        std::os::unix::net::UnixListener::bind(path).map_err(|e| BindError::Io(format!("{}: {e}", path.display())))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| BindError::Io(format!("{}: {e}", path.display())))?;
    listener.set_nonblocking(true).map_err(|e| BindError::Io(e.to_string()))?;
    Ok(listener)
}

/// Sends one request and waits for the reply (blocking; used by the CLI and the MCP server).
pub fn request(path: &Path, req: &Request, timeout: Duration) -> Result<Reply, String> {
    let mut stream = UnixStream::connect(path).map_err(|_| "Claude Notch is not running".to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream.take(MAX_MESSAGE)).read_line(&mut reply).map_err(|e| e.to_string())?;
    serde_json::from_str(&reply).map_err(|e| format!("bad reply from Claude Notch: {e}"))
}

/// Serves connections until `incoming` closes.
pub async fn serve(listener: std::os::unix::net::UnixListener, incoming: mpsc::UnboundedSender<Incoming>) {
    let listener = match tokio::net::UnixListener::from_std(listener) {
        Ok(l) => l,
        Err(e) => {
            crate::log(&format!("ipc: {e}"));
            return;
        }
    };
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    while !incoming.is_closed() {
        let Ok((stream, _)) = listener.accept().await else { continue };
        // The socket is 0600 inside a 0700 directory already; this also covers a lax umask or ACL.
        if stream.peer_cred().ok().map(|c| c.uid()) != Some(uid) {
            continue;
        }
        tokio::spawn(handle(stream, incoming.clone()));
    }
}

async fn handle(stream: tokio::net::UnixStream, incoming: mpsc::UnboundedSender<Incoming>) {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let read = tokio::io::BufReader::new(read.take(MAX_MESSAGE)).read_line(&mut line).await;
    let simple = |msg| Reply { ok: incoming.send(msg).is_ok(), ..Reply::default() };
    let reply = match read
        .map_err(|e| e.to_string())
        .and_then(|_| serde_json::from_str::<Request>(&line).map_err(|e| format!("bad request: {e}")))
    {
        Err(e) => Reply { error: Some(e), ..Reply::default() },
        Ok(Request::Listen) => simple(Incoming::Listen),
        Ok(Request::Cancel) => simple(Incoming::Cancel),
        Ok(Request::Pause) => simple(Incoming::Pause),
        Ok(Request::Settings { tab }) => simple(Incoming::Settings(tab)),
        Ok(Request::Ask { text }) if text.trim().is_empty() || text.chars().count() > MAX_ASK => {
            Reply { error: Some(format!("the command must have 1 to {MAX_ASK} characters")), ..Reply::default() }
        }
        Ok(Request::Ask { text }) => simple(Incoming::Ask(text.trim().to_string())),
        Ok(Request::Demo { mode }) => simple(Incoming::Demo(mode)),
        Ok(Request::Approve { tool_name, input }) => {
            let (tx, rx) = oneshot::channel();
            let sent = incoming.send(Incoming::Approve { tool_name, input, reply: tx }).is_ok();
            // A dropped sender (run ended, app quitting) means no.
            let allow = sent && rx.await.unwrap_or(false);
            Reply { ok: true, allow: Some(allow), error: None }
        }
    };
    if let Ok(mut out) = serde_json::to_string(&reply) {
        out.push('\n');
        let _ = write.write_all(out.as_bytes()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("claude-notch-{name}-{}.sock", std::process::id()))
    }

    #[test]
    fn request_wire_format() {
        assert_eq!(serde_json::to_string(&Request::Listen).unwrap(), r#"{"cmd":"listen"}"#);
        let req: Request =
            serde_json::from_str(r#"{"cmd":"approve","tool_name":"Bash","input":{"command":"ls"}}"#).unwrap();
        assert_eq!(req, Request::Approve { tool_name: "Bash".into(), input: serde_json::json!({"command": "ls"}) });
        // serde enforces deny_unknown_fields on the variants with fields.
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"demo","mode":"idle","extra":1}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"demo","mode":"bogus"}"#).is_err());
        let ask: Request = serde_json::from_str(r#"{"cmd":"ask","text":"open youtube"}"#).unwrap();
        assert_eq!(ask, Request::Ask { text: "open youtube".into() });
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"approve","tool_name":"Bash"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"rm"}"#).is_err());
        assert!(serde_json::from_str::<Request>("{}").is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn round_trip_commands_and_approvals() {
        let path = socket("rt");
        let listener = bind(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(matches!(bind(&path), Err(BindError::AlreadyRunning)));

        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(serve(listener, tx));

        let p = path.clone();
        let reply = tokio::task::spawn_blocking(move || request(&p, &Request::Listen, Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();
        assert!(reply.ok);
        assert!(matches!(rx.recv().await, Some(Incoming::Listen)));

        for answer in [true, false] {
            let p = path.clone();
            let asking = tokio::task::spawn_blocking(move || {
                let req = Request::Approve { tool_name: "Bash".into(), input: serde_json::json!({"command": "ls"}) };
                request(&p, &req, Duration::from_secs(2))
            });
            let Some(Incoming::Approve { tool_name, reply, .. }) = rx.recv().await else { panic!("no approval") };
            assert_eq!(tool_name, "Bash");
            reply.send(answer).unwrap();
            assert_eq!(asking.await.unwrap().unwrap().allow, Some(answer));
        }

        // The app dropping the request means deny.
        let p = path.clone();
        let asking = tokio::task::spawn_blocking(move || {
            request(&p, &Request::Approve { tool_name: "Bash".into(), input: Value::Null }, Duration::from_secs(2))
        });
        drop(rx.recv().await);
        assert_eq!(asking.await.unwrap().unwrap().allow, Some(false));

        let p = path.clone();
        let raw = tokio::task::spawn_blocking(move || {
            let mut s = UnixStream::connect(&p).unwrap();
            s.write_all(b"{\"cmd\":\"nope\"}\n").unwrap();
            let mut out = String::new();
            BufReader::new(s).read_line(&mut out).unwrap();
            out
        });
        let reply: Reply = serde_json::from_str(&raw.await.unwrap()).unwrap();
        assert!(!reply.ok && reply.error.unwrap().starts_with("bad request"));

        for (text, ok) in [("  open youtube ", true), ("   ", false), (&*"x".repeat(MAX_ASK + 1), false)] {
            let (p, req) = (path.clone(), Request::Ask { text: text.into() });
            let reply = tokio::task::spawn_blocking(move || request(&p, &req, Duration::from_secs(2))).await.unwrap();
            assert_eq!(reply.unwrap().ok, ok, "{text:?}");
            if ok {
                assert!(matches!(rx.recv().await, Some(Incoming::Ask(t)) if t == "open youtube"));
            }
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn stale_socket_is_replaced_and_missing_app_is_reported() {
        let path = socket("stale");
        drop(bind(&path).unwrap());
        let listener = bind(&path).unwrap();
        drop(listener);
        let err = request(&path, &Request::Listen, Duration::from_secs(1)).unwrap_err();
        assert_eq!(err, "Claude Notch is not running");
        std::fs::remove_file(&path).unwrap();
    }
}
