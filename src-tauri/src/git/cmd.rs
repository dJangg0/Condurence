//! Thin async wrapper around the `git` CLI. Each call inherits the hydrated PATH.

use std::path::Path;
use std::process::Output;

use tokio::process::Command;

use crate::error::{AppError, AppResult};

/// Runs `git <args>` in `cwd` and returns the raw output regardless of exit status.
pub async fn raw(cwd: &Path, args: &[&str]) -> AppResult<Output> {
    Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| AppError::msg(format!("failed to run git: {e}")))
}

/// Runs `git <args>` and returns stdout, or an error carrying stderr on failure.
pub async fn run(cwd: &Path, args: &[&str]) -> AppResult<String> {
    let output = raw(cwd, args).await?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(AppError::msg(format!(
            "git {}: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Blocking variant for dedicated threads such as the file watcher.
pub fn run_sync(cwd: &Path, args: &[&str], optional_locks: bool) -> AppResult<String> {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    if optional_locks {
        // Read-only status polling must never contend with the user's own git commands.
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    let output = command
        .output()
        .map_err(|e| AppError::msg(format!("failed to run git: {e}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(AppError::msg(String::from_utf8_lossy(&output.stderr).trim().to_string()))
    }
}
