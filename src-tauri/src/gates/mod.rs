//! Deterministic gates: the repository's own checks, run by the runtime when a delegated agent
//! reports `completed`. An agent's report is a claim. The gates' exit codes are the evidence, and a
//! failing gate overrides the claim.
//!
//! Configured per repository in `.orchestrator/gates.json`:
//!
//! ```json
//! { "gates": [ { "id": "test", "command": "npm test", "timeoutSec": 900 } ] }
//! ```
//!
//! Gates run in order in the child's worktree and stop at the first failure. With no file, there
//! are no gates and a report is taken as it was written.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};

pub const GATES_FILE: &str = ".orchestrator/gates.json";
const DEFAULT_TIMEOUT_SECS: u64 = 900;
const MAX_TIMEOUT_SECS: u64 = 3600;
const MAX_GATE_ID_LEN: usize = 40;
/// The last part of a gate's output, kept for the agent and the run log.
const OUTPUT_TAIL_CHARS: usize = 2000;
/// How long a timed-out or cancelled gate gets to exit after SIGTERM, before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    pub id: String,
    pub command: String,
    #[serde(default = "default_timeout", rename = "timeoutSec")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Deserialize)]
struct GatesFile {
    #[serde(default)]
    gates: Vec<Gate>,
}

/// Parses a gates file. Ids are short and unique, commands are not empty, and timeouts are sane.
pub fn parse(text: &str) -> AppResult<Vec<Gate>> {
    let file: GatesFile = serde_json::from_str(text)
        .map_err(|error| AppError::msg(format!("{GATES_FILE} is not valid JSON: {error}")))?;
    let mut seen = HashSet::new();
    for gate in &file.gates {
        let id_ok = !gate.id.is_empty()
            && gate.id.len() <= MAX_GATE_ID_LEN
            && gate.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !id_ok {
            crate::error::bail!("gate id '{}' must be 1-{MAX_GATE_ID_LEN} letters, digits, '-' or '_'", gate.id);
        }
        if !seen.insert(gate.id.as_str()) {
            crate::error::bail!("gate id '{}' is used twice in {GATES_FILE}", gate.id);
        }
        if gate.command.trim().is_empty() {
            crate::error::bail!("gate '{}' needs a command", gate.id);
        }
        if gate.timeout_secs == 0 || gate.timeout_secs > MAX_TIMEOUT_SECS {
            crate::error::bail!("gate '{}' timeoutSec must be between 1 and {MAX_TIMEOUT_SECS}", gate.id);
        }
    }
    Ok(file.gates)
}

/// The repository's gates, or none when it has no gates file.
pub fn load(repo_root: &Path) -> AppResult<Vec<Gate>> {
    match std::fs::read_to_string(repo_root.join(GATES_FILE)) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(AppError::msg(format!("cannot read {GATES_FILE}: {error}"))),
    }
}

/// How one gate ended. `passed` is the only verdict the runtime acts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateResult {
    pub id: String,
    pub command: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_tail: String,
}

/// Runs the gates in order in `cwd`, stopping at the first failure: later gates would only repeat
/// the news. Output goes to a file in `scratch`, which is removed once read. Cancelling the run
/// stops the gate that is running, and no further gates start.
pub async fn run_all(gates: &[Gate], cwd: &Path, scratch: &Path, cancel: &CancellationToken) -> Vec<GateResult> {
    let mut results = Vec::new();
    for (index, gate) in gates.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        let result = run_one(gate, index, cwd, scratch, cancel).await;
        let passed = result.passed;
        results.push(result);
        if !passed {
            break;
        }
    }
    results
}

enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

async fn run_one(gate: &Gate, index: usize, cwd: &Path, scratch: &Path, cancel: &CancellationToken) -> GateResult {
    let failed = |output_tail: String, timed_out: bool| GateResult {
        id: gate.id.clone(),
        command: gate.command.clone(),
        passed: false,
        exit_code: None,
        timed_out,
        output_tail,
    };

    let log_path: PathBuf = scratch.join(format!("gate-{index}-{}.log", gate.id));
    let file = match std::fs::File::create(&log_path) {
        Ok(file) => file,
        Err(error) => return failed(format!("cannot create the gate's log file: {error}"), false),
    };
    let copy = match file.try_clone() {
        Ok(copy) => copy,
        Err(error) => return failed(format!("cannot create the gate's log file: {error}"), false),
    };

    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(&gate.command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(copy))
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_file(&log_path);
            return failed(format!("cannot start the gate: {error}"), false);
        }
    };
    let pid = child.id();

    let ended = tokio::select! {
        status = child.wait() => Ended::Exited(status.ok().and_then(|status| status.code())),
        _ = tokio::time::sleep(Duration::from_secs(gate.timeout_secs)) => Ended::TimedOut,
        _ = cancel.cancelled() => Ended::Cancelled,
    };
    let (exit_code, timed_out) = match ended {
        Ended::Exited(code) => (code, false),
        Ended::TimedOut => {
            stop_group(pid).await;
            let _ = child.wait().await;
            (None, true)
        }
        Ended::Cancelled => {
            stop_group(pid).await;
            let _ = child.wait().await;
            (None, false)
        }
    };

    let output_tail = read_tail(&log_path);
    let _ = std::fs::remove_file(&log_path);
    GateResult {
        id: gate.id.clone(),
        command: gate.command.clone(),
        passed: exit_code == Some(0) && !timed_out,
        exit_code,
        timed_out,
        output_tail,
    }
}

/// Stops the gate's whole process group, so a test runner's workers do not outlive it.
async fn stop_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || crate::pty::process_group::terminate_tree(pid, STOP_GRACE)).await;
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn read_tail(path: &Path) -> String {
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return String::new(),
    };
    let count = text.chars().count();
    if count > OUTPUT_TAIL_CHARS {
        text.chars().skip(count - OUTPUT_TAIL_CHARS).collect()
    } else {
        text
    }
}

/// What went wrong, stable enough to recognise the same failure twice: the gate, how it ended, and
/// its last non-empty output line with digits masked, so timings and counts do not matter.
pub fn signature(result: &GateResult) -> String {
    if result.timed_out {
        return format!("gate:{}:timeout", result.id);
    }
    let how = match result.exit_code {
        Some(code) => format!("exit{code}"),
        None => "killed".to_string(),
    };
    let last_line = result.output_tail.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("");
    format!("gate:{}:{how}:{}", result.id, normalize(last_line))
}

/// Lowercases, masks digits and collapses whitespace, so two runs of the same failure compare equal.
pub fn normalize(text: &str) -> String {
    let masked: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect();
    masked.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(id: &str, command: &str) -> Gate {
        Gate {
            id: id.into(),
            command: command.into(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    #[test]
    fn parses_gates_and_defaults_the_timeout() {
        let gates = parse(r#"{"gates":[{"id":"test","command":"npm test"}]}"#).unwrap();
        assert_eq!(gates, vec![gate("test", "npm test")]);
    }

    #[test]
    fn a_file_without_gates_means_none() {
        assert!(parse("{}").unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_gates() {
        assert!(parse(r#"{"gates":[{"id":"has space","command":"x"}]}"#).is_err());
        assert!(parse(r#"{"gates":[{"id":"a","command":"x"},{"id":"a","command":"y"}]}"#).is_err());
        assert!(parse(r#"{"gates":[{"id":"a","command":"   "}]}"#).is_err());
        assert!(parse(r#"{"gates":[{"id":"a","command":"x","timeoutSec":0}]}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn a_missing_file_means_no_gates() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn signatures_ignore_digits_and_compare_equal_for_the_same_failure() {
        let first = GateResult {
            id: "test".into(),
            command: "npm test".into(),
            passed: false,
            exit_code: Some(1),
            timed_out: false,
            output_tail: "ok\nFAIL  src/a.test.ts (12 ms)\nAssertionError: expected 3, got 2\n".into(),
        };
        let again = GateResult {
            output_tail: "FAIL  src/a.test.ts (97 ms)\nAssertionError: expected 4, got 5\n".into(),
            ..first.clone()
        };
        assert_eq!(signature(&first), signature(&again));
        assert!(signature(&first).starts_with("gate:test:exit1:"));
        let other = GateResult {
            output_tail: "SyntaxError: unexpected token".into(),
            ..first.clone()
        };
        assert_ne!(signature(&first), signature(&other));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_gates_in_order_and_stops_at_the_first_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let gates = vec![gate("pass", "echo fine"), gate("fail", "echo broken; exit 3"), gate("never", "echo nope")];
        let results = run_all(&gates, dir.path(), dir.path(), &cancel).await;
        assert_eq!(results.len(), 2, "the gate after the failure must not run");
        assert!(results[0].passed);
        assert!(results[0].output_tail.contains("fine"));
        assert!(!results[1].passed);
        assert_eq!(results[1].exit_code, Some(3));
        assert!(results[1].output_tail.contains("broken"));
        // Every gate's log is removed once read.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_gate_that_runs_too_long_is_stopped_and_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let mut slow = gate("slow", "sleep 30");
        slow.timeout_secs = 1;
        let started = std::time::Instant::now();
        let results = run_all(&[slow], dir.path(), dir.path(), &cancel).await;
        assert!(started.elapsed() < Duration::from_secs(10), "the gate should be stopped at its timeout");
        assert!(results[0].timed_out);
        assert!(!results[0].passed);
        assert_eq!(signature(&results[0]), "gate:slow:timeout");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_stops_the_running_gate() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        let results = run_all(&[gate("long", "sleep 30")], dir.path(), dir.path(), &cancel).await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
    }
}
