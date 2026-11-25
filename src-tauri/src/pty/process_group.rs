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

/// Windows: `taskkill /T` walks the process tree. Job-object based containment (so that
/// even detached grandchildren die with the app) is tracked as a follow-up.
#[cfg(windows)]
pub fn terminate_tree(pid: u32, _grace: Duration) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}
