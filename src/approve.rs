//! `claude-notch mcp-approve`: the MCP server behind `--permission-prompt-tool`. Claude Code
//! calls it before any tool the allowlist does not cover; it forwards the request to the
//! running notch, which shows Allow / Deny, and returns the user's click. No answer means no.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};

use crate::ipc::{self, Request};

const TOOL: &str = "approve";
/// Claude Code's own MCP tool timeout is longer; the user gets this long to click.
const WAIT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allow: bool,
    pub message: String,
}

/// The reply Claude Code expects from a permission prompt tool.
fn decision_text(decision: &Decision, input: &Value) -> String {
    if decision.allow {
        json!({ "behavior": "allow", "updatedInput": input }).to_string()
    } else {
        json!({ "behavior": "deny", "message": decision.message }).to_string()
    }
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Answers one JSON-RPC message; `None` for notifications, which get no reply.
pub fn handle(msg: &Value, decide: &mut dyn FnMut(&str, &Value) -> Decision) -> Option<Value> {
    let id = msg.get("id")?;
    let result = match msg["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "claude-notch", "version": env!("CARGO_PKG_VERSION") },
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({ "tools": [{
            "name": TOOL,
            "description": "Asks the user on the Claude Notch whether a tool may run.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tool_name": { "type": "string" },
                    "input": { "type": "object" },
                    "tool_use_id": { "type": "string" },
                },
                "required": ["tool_name", "input"],
            },
        }]}),
        Some("tools/call") => {
            let params = &msg["params"];
            if params["name"] != TOOL {
                return Some(error(id, -32602, "unknown tool"));
            }
            let Some(tool_name) = params["arguments"]["tool_name"].as_str() else {
                return Some(error(id, -32602, "tool_name is required"));
            };
            let input = params["arguments"].get("input").cloned().unwrap_or_else(|| json!({}));
            let decision = decide(tool_name, &input);
            json!({ "content": [{ "type": "text", "text": decision_text(&decision, &input) }] })
        }
        _ => return Some(error(id, -32601, "method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn ask_notch(tool_name: &str, input: &Value) -> Decision {
    ask(crate::config::socket_path(), tool_name, input)
}

/// Asks the notch at `socket`; any failure is a denial with the reason.
fn ask(socket: Result<std::path::PathBuf, String>, tool_name: &str, input: &Value) -> Decision {
    let req = Request::Approve { tool_name: tool_name.to_string(), input: input.clone() };
    let reply = socket.and_then(|path| ipc::request(&path, &req, WAIT));
    match reply {
        Ok(reply) if reply.allow == Some(true) => Decision { allow: true, message: String::new() },
        Ok(_) => Decision { allow: false, message: "The user denied this on the Claude Notch.".into() },
        Err(e) => Decision { allow: false, message: format!("No approval: {e}.") },
    }
}

/// Serves stdin/stdout until Claude Code closes stdin.
pub fn serve_stdio() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return 1 };
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, &mut ask_notch),
            Err(_) => Some(error(&Value::Null, -32700, "parse error")),
        };
        if let Some(reply) = reply
            && writeln!(stdout, "{reply}").and_then(|()| stdout.flush()).is_err()
        {
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(args: &Value, allow: bool) -> Value {
        let msg =
            json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": TOOL, "arguments": args } });
        let mut seen = None;
        let reply = handle(&msg, &mut |tool, input| {
            seen = Some((tool.to_string(), input.clone()));
            Decision { allow, message: "nope".into() }
        })
        .unwrap();
        assert_eq!(seen.unwrap().0, "Bash");
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    fn never(_: &str, _: &Value) -> Decision {
        panic!("decide must not be called")
    }

    #[test]
    fn initialize_and_list() {
        let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25"}});
        let reply = handle(&init, &mut never).unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(reply["id"], 0);
        let list = handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}), &mut never).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], TOOL);
    }

    #[test]
    fn notifications_get_no_reply() {
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), &mut never).is_none());
    }

    #[test]
    fn allow_echoes_the_input_unchanged() {
        let input = json!({"command": "touch x", "description": "d"});
        let out = call(&json!({"tool_name": "Bash", "input": input, "tool_use_id": "t"}), true);
        assert_eq!(out, json!({"behavior": "allow", "updatedInput": input}));
    }

    #[test]
    fn deny_carries_the_message() {
        let out = call(&json!({"tool_name": "Bash", "input": {"command": "rm -rf ~"}}), false);
        assert_eq!(out, json!({"behavior": "deny", "message": "nope"}));
    }

    #[test]
    fn bad_calls_are_errors_not_approvals() {
        let wrong_tool = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"other","arguments":{}}});
        assert_eq!(handle(&wrong_tool, &mut never).unwrap()["error"]["code"], -32602);
        let no_name = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":TOOL,"arguments":{}}});
        assert_eq!(handle(&no_name, &mut never).unwrap()["error"]["code"], -32602);
        let unknown = json!({"jsonrpc":"2.0","id":4,"method":"resources/list"});
        assert_eq!(handle(&unknown, &mut never).unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn no_running_notch_means_deny() {
        let missing = ask(Ok("/nonexistent/claude-notch.sock".into()), "Bash", &json!({}));
        assert_eq!(missing, Decision { allow: false, message: "No approval: Claude Notch is not running.".into() });
        let no_runtime_dir = ask(Err("XDG_RUNTIME_DIR is not set".into()), "Bash", &json!({}));
        assert!(!no_runtime_dir.allow);
    }
}
