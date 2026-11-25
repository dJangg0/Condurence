//! `--mcp-bridge` mode: a stdio JSON-RPC 2.0 proxy.
//!
//! Provider CLIs speak MCP over stdio. Each line they send is forwarded verbatim to the
//! in-process loopback server, and the server's reply is written back to stdout. This lets
//! unmodified third-party CLIs call into the orchestrator without OS-specific socket tricks.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

pub struct BridgeConfig {
    pub port: u16,
    pub run_id: String,
    pub instance_id: String,
    pub token: String,
}

/// Runs bridge mode when `--mcp-bridge` is the first argument. Returns the process exit
/// code, or `None` when the app should start normally.
pub fn run_if_requested(args: impl IntoIterator<Item = String>) -> Option<i32> {
    let args: Vec<String> = args.into_iter().collect();
    if args.first().map(String::as_str) != Some("--mcp-bridge") {
        return None;
    }
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let config = match (
        value("--port").and_then(|p| p.parse::<u16>().ok()),
        value("--run-id"),
        value("--instance-id"),
        value("--token"),
    ) {
        (Some(port), Some(run_id), Some(instance_id), Some(token)) => BridgeConfig {
            port,
            run_id,
            instance_id,
            token,
        },
        _ => {
            eprintln!("Condurence --mcp-bridge: missing --port/--run-id/--instance-id/--token");
            return Some(2);
        }
    };
    Some(run(config))
}

pub fn run(config: BridgeConfig) -> i32 {
    let url = format!(
        "http://127.0.0.1:{}/mcp/{}/{}",
        config.port, config.run_id, config.instance_id
    );
    let auth = format!("Bearer {}", config.token);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(e) => {
                let reply = rpc_error(Value::Null, -32700, &format!("parse error: {e}"));
                let _ = writeln!(stdout, "{reply}");
                let _ = stdout.flush();
                continue;
            }
        };
        // Notifications carry no id and must not produce a reply.
        let id = request.get("id").cloned();

        let reply = match forward(&agent, &url, &auth, line) {
            Ok(Some(body)) => Some(body),
            Ok(None) => id.clone().map(|id| rpc_error(id, -32603, "orchestrator returned no reply")),
            Err(message) => id.clone().map(|id| rpc_error(id, -32603, &message)),
        };
        if let Some(reply) = reply {
            if id.is_some() {
                let _ = writeln!(stdout, "{reply}");
                let _ = stdout.flush();
            }
        }
    }
    0
}

/// POSTs one JSON-RPC message. `Ok(None)` means the server accepted it with no body.
fn forward(agent: &ureq::Agent, url: &str, auth: &str, body: &str) -> Result<Option<String>, String> {
    let mut response = agent
        .post(url)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .send(body)
        .map_err(|e| format!("orchestrator unreachable: {e}"))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("failed to read orchestrator reply: {e}"))?;
    match status {
        200..=299 if text.trim().is_empty() => Ok(None),
        200..=299 => Ok(Some(text.trim().to_string())),
        _ => Err(format!("orchestrator rejected the request (HTTP {status}): {text}")),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}
