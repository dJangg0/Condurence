//! The stdio bridge must not let one long tool call block the requests behind it. A root that
//! delegates to two agents in the same turn sends two `tools/call` messages back to back, and the
//! second child should start while the first is still working. JSON-RPC lets replies arrive in
//! any order, matched by id.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Answers every POST with an empty result, after a delay when the body asks for the slow method.
fn serve_mock_orchestrator(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || answer(stream));
    }
}

fn answer(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if request["method"] == "slow" {
        std::thread::sleep(Duration::from_millis(1500));
    }
    let reply = json!({ "jsonrpc": "2.0", "id": request["id"], "result": {} }).to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.len(),
        reply
    );
}

#[test]
fn a_slow_request_does_not_hold_up_the_ones_behind_it() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || serve_mock_orchestrator(listener));

    let mut bridge = Command::new(env!("CARGO_BIN_EXE_Condurence"))
        .args(["--mcp-bridge", "--port", &port.to_string(), "--run-id", "r", "--instance-id", "i", "--token", "t"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let stdout = bridge.stdout.take().unwrap();

    let started = Instant::now();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "slow" })).unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 2, "method": "fast" })).unwrap();
    stdin.flush().unwrap();

    let mut lines = BufReader::new(stdout).lines();
    let first: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let first_at = started.elapsed();
    let second: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();

    assert_eq!(first["id"], 2, "the fast request should be answered first, was {first}");
    assert!(first_at < Duration::from_millis(1200), "the fast request waited {first_at:?} behind the slow one");
    assert_eq!(second["id"], 1);

    // Closing stdin is how the CLI says it is done. The bridge must then exit on its own.
    drop(stdin);
    assert!(bridge.wait().unwrap().success());
}

#[test]
fn closing_stdin_still_waits_for_requests_already_in_flight() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || serve_mock_orchestrator(listener));

    let mut bridge = Command::new(env!("CARGO_BIN_EXE_Condurence"))
        .args(["--mcp-bridge", "--port", &port.to_string(), "--run-id", "r", "--instance-id", "i", "--token", "t"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let stdout = bridge.stdout.take().unwrap();

    // One-shot use: send a request and close stdin straight away. The reply must still be written.
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 7, "method": "slow" })).unwrap();
    drop(stdin);

    let mut lines = BufReader::new(stdout).lines();
    let reply: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(reply["id"], 7);
    assert!(bridge.wait().unwrap().success());
}
