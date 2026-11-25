//! PTY session manager built on `portable-pty`.
//!
//! Each session owns a master end, a 256 KB replay ring and an exit watch channel. A reader
//! thread streams UTF-8-safe chunks over `pty-output://{id}`, and a waiter thread publishes
//! the exit code over `pty-exit://{id}` and to any task awaiting completion.

use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use dashmap::DashMap;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use tokio::sync::watch;

use super::process_group;
use super::ring::{RingBuffer, RING_CAPACITY};
use crate::error::{AppError, AppResult};
use crate::runtime::events::EventSink;

const TERMINATE_GRACE: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug)]
pub struct SpawnSpec {
    pub session_id: String,
    pub cwd: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

pub struct PtySession {
    pub id: String,
    pub cwd: PathBuf,
    pub pid: Option<u32>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    ring: Mutex<RingBuffer>,
    exit: watch::Sender<Option<i32>>,
}

impl PtySession {
    pub fn write(&self, data: &[u8]) -> AppResult<()> {
        let mut writer = lock(&self.writer);
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        lock(&self.master)
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_err)
    }

    /// Replays buffered output (lossy UTF-8, ANSI sequences preserved for xterm.js).
    pub fn history(&self) -> String {
        String::from_utf8_lossy(&lock(&self.ring).snapshot()).into_owned()
    }

    pub fn subscribe_exit(&self) -> watch::Receiver<Option<i32>> {
        self.exit.subscribe()
    }

    pub fn exit_code(&self) -> Option<i32> {
        *self.exit.borrow()
    }

    pub fn is_running(&self) -> bool {
        self.exit_code().is_none()
    }
}

pub struct PtyManager {
    events: Arc<dyn EventSink>,
    sessions: DashMap<String, Arc<PtySession>>,
}

impl PtyManager {
    pub fn new(events: Arc<dyn EventSink>) -> Self {
        Self {
            events,
            sessions: DashMap::new(),
        }
    }

    pub fn spawn(&self, spec: SpawnSpec) -> AppResult<Arc<PtySession>> {
        if self.sessions.contains_key(&spec.session_id) {
            crate::error::bail!("session '{}' already exists", spec.session_id);
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows.max(1),
                cols: spec.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_err)?;

        let (program, args) = launch_argv(&spec.program, &spec.args);
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(&spec.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        for (key, value) in &spec.env {
            command.env(key, value);
        }

        let child = pair.slave.spawn_command(command).map_err(io_err)?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(io_err)?;
        let writer = pair.master.take_writer().map_err(io_err)?;
        let pid = child.process_id();

        let (exit_tx, _) = watch::channel(None::<i32>);
        let session = Arc::new(PtySession {
            id: spec.session_id.clone(),
            cwd: spec.cwd.clone(),
            pid,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            ring: Mutex::new(RingBuffer::new(RING_CAPACITY)),
            exit: exit_tx,
        });
        self.sessions.insert(spec.session_id, session.clone());

        spawn_reader(self.events.clone(), session.clone(), reader);
        spawn_waiter(self.events.clone(), session.clone(), child);
        Ok(session)
    }

    pub fn get(&self, session_id: &str) -> AppResult<Arc<PtySession>> {
        self.sessions
            .get(session_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| AppError::msg(format!("no terminal session '{session_id}'")))
    }

    /// Blocking: sends SIGTERM to the process group and escalates after a grace period.
    pub fn terminate(&self, session_id: &str) {
        if let Some(session) = self.sessions.get(session_id).map(|e| e.value().clone()) {
            if let Some(pid) = session.pid {
                process_group::terminate_tree(pid, TERMINATE_GRACE);
            }
        }
    }

    pub fn terminate_all(&self) {
        let ids: Vec<String> = self.sessions.iter().map(|e| e.key().clone()).collect();
        for id in ids {
            self.terminate(&id);
        }
    }
}

fn spawn_reader(events: Arc<dyn EventSink>, session: Arc<PtySession>, mut reader: Box<dyn Read + Send>) {
    let event = format!("pty-output://{}", session.id);
    let thread_name = format!("pty-read-{}", short(&session.id));
    let spawned = std::thread::Builder::new().name(thread_name).spawn(move || {
        let mut buffer = vec![0u8; 8192];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    lock(&session.ring).push(&buffer[..n]);
                    carry.extend_from_slice(&buffer[..n]);
                    loop {
                        let complete = complete_utf8_prefix(&carry);
                        if complete == 0 {
                            break;
                        }
                        let chunk = String::from_utf8_lossy(&carry[..complete]).into_owned();
                        carry.drain(..complete);
                        events.emit(&event, Value::String(chunk));
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if !carry.is_empty() {
            events.emit(&event, Value::String(String::from_utf8_lossy(&carry).into_owned()));
        }
    });
    if let Err(e) = spawned {
        eprintln!("failed to start PTY reader thread: {e}");
    }
}

fn spawn_waiter(events: Arc<dyn EventSink>, session: Arc<PtySession>, mut child: Box<dyn Child + Send + Sync>) {
    let thread_name = format!("pty-wait-{}", short(&session.id));
    let spawned = std::thread::Builder::new().name(thread_name).spawn(move || {
        let code = match child.wait() {
            Ok(status) => status.exit_code() as i32,
            Err(_) => -1,
        };
        session.exit.send_replace(Some(code));
        events.emit(&format!("pty-exit://{}", session.id), json!({ "exitCode": code }));
    });
    if let Err(e) = spawned {
        eprintln!("failed to start PTY waiter thread: {e}");
    }
}

/// Length of the longest prefix that is valid UTF-8 and can be emitted now. Invalid
/// sequences are consumed (replacement char later); an incomplete trailing sequence is
/// held back until the next read completes it.
fn complete_utf8_prefix(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) => match e.error_len() {
            Some(invalid) => e.valid_up_to() + invalid,
            None => e.valid_up_to(),
        },
    }
}

/// On Windows, npm-installed CLIs are `.cmd` shims that CreateProcess cannot launch
/// directly, so they are routed through `cmd.exe /c`.
#[cfg(windows)]
fn launch_argv(program: &str, args: &[String]) -> (String, Vec<String>) {
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        let mut argv = vec!["/d".to_string(), "/s".to_string(), "/c".to_string(), program.to_string()];
        argv.extend(args.iter().cloned());
        return ("cmd.exe".to_string(), argv);
    }
    (program.to_string(), args.to_vec())
}

#[cfg(not(windows))]
fn launch_argv(program: &str, args: &[String]) -> (String, Vec<String>) {
    (program.to_string(), args.to_vec())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicked reader must not wedge input or history access for the whole app.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn io_err(error: impl std::fmt::Display) -> AppError {
    AppError::msg(error.to_string())
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_back_incomplete_utf8_tail() {
        // "é" is 0xC3 0xA9; split across two reads.
        assert_eq!(complete_utf8_prefix(b"ab\xC3"), 2);
        assert_eq!(complete_utf8_prefix(b"ab\xC3\xA9"), 4);
    }

    #[test]
    fn consumes_invalid_bytes_without_stalling() {
        assert_eq!(complete_utf8_prefix(b"a\xFFb"), 2);
    }
}
