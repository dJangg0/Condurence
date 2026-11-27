//! OS-level process-tree teardown so closing a workspace never leaves compilers, test
//! runners or dev servers orphaned.

use std::time::Duration;

/// Terminates the process group whose leader is `pid`: SIGTERM first, then SIGKILL after
/// `grace`. Each PTY child is started with `setsid`, so its pid is also its group id.
#[cfg(unix)]
pub fn terminate_tree(pid: u32, grace: Duration) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    use std::time::Instant;

    let group = Pid::from_raw(pid as i32);
    let _ = killpg(group, Signal::SIGTERM);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        // Signal 0 probes existence without delivering anything.
        if killpg(group, None).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = killpg(group, Signal::SIGKILL);
}

/// Cleans up after a session's leader has exited: anything it left running in its process group
/// (a backgrounded dev server, a test watcher) would otherwise outlive the session. Returns whether
/// there was anything to stop. The group id stays valid while any member lives, so a group that
/// has fully gone is simply reported as empty.
#[cfg(unix)]
pub fn sweep_group(pid: u32, grace: Duration) -> bool {
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;

    if killpg(Pid::from_raw(pid as i32), None).is_err() {
        return false;
    }
    terminate_tree(pid, grace);
    true
}

/// Windows: the leader's tree is already gone by the time this runs, and `taskkill /T` cannot find
/// descendants of a dead parent, so there is nothing left that can be reached.
#[cfg(windows)]
pub fn sweep_group(_pid: u32, _grace: Duration) -> bool {
    false
}

/// Windows: `taskkill /T` walks the process tree. Job-object based containment (so that
/// even detached grandchildren die with the app) is tracked as a follow-up.
#[cfg(windows)]
pub fn terminate_tree(pid: u32, _grace: Duration) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

/// Who the process with `pid` is, as the pid plus the moment it started. A pid can be reused by an
/// unrelated process after the original exits, but not at the same start time, so a stored value
/// that still matches identifies the same process. `None` if no such process exists.
#[cfg(unix)]
pub fn process_identity(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(windows)]
pub fn process_identity(_pid: u32) -> Option<String> {
    None
}

/// Whether `pid` leads its own process group, as every PTY session's process does. Killing a group
/// is only safe when the group is the one this session created.
#[cfg(unix)]
pub fn is_group_leader(pid: u32) -> bool {
    use nix::unistd::{getpgid, Pid};
    let pid = Pid::from_raw(pid as i32);
    getpgid(Some(pid)).map(|group| group == pid).unwrap_or(false)
}

#[cfg(windows)]
pub fn is_group_leader(_pid: u32) -> bool {
    false
}
