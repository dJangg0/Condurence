//! Cross-platform PATH hydration and CLI binary discovery.
//!
//! macOS GUI apps launched from Finder/Dock receive a minimal PATH without
//! Homebrew, NVM, Volta or Cargo. We run the user's login shell once at startup
//! and import its environment, so git, PTYs and provider CLIs resolve exactly as
//! they do in the user's terminal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::time::Duration;

#[cfg(any(unix, test))]
const BEGIN: &str = "__OC_ENV_BEGIN__";
#[cfg(any(unix, test))]
const END: &str = "__OC_ENV_END__";
/// Shell-local variables that must not be copied into child processes.
const SHELL_LOCAL: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_"];

/// Imports the login-shell environment into this process.
///
/// Must be called once, before any threads are spawned, because it mutates the
/// process environment.
pub fn hydrate_process_env() {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    if let Some(shell_env) = login_shell_env() {
        env.extend(shell_env);
    }

    let mut path_dirs: Vec<PathBuf> = env
        .get("PATH")
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for dir in fallback_dirs(&env) {
        if dir.is_dir() && !path_dirs.contains(&dir) {
            path_dirs.push(dir);
        }
    }
    if let Ok(joined) = std::env::join_paths(&path_dirs) {
        env.insert("PATH".into(), joined.to_string_lossy().into_owned());
    }

    for (key, value) in env {
        if SHELL_LOCAL.contains(&key.as_str()) {
            continue;
        }
        std::env::set_var(&key, &value);
    }
}

/// Resolves a CLI name (`claude`, `codex`, ...) against the hydrated PATH.
/// On Windows this also honours PATHEXT, so `.cmd` npm shims are found.
pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

/// Resolves a provider CLI. `OPENCONDUCTOR_<NAME>_BIN` (e.g. `OPENCONDUCTOR_CLAUDE_BIN`)
/// pins an explicit executable, for custom installs and for tests.
pub fn resolve_provider(name: &str) -> Option<PathBuf> {
    let var = format!("OPENCONDUCTOR_{}_BIN", name.to_ascii_uppercase());
    match std::env::var_os(&var).map(PathBuf::from) {
        Some(path) if path.is_file() => Some(path),
        _ => resolve_binary(name),
    }
}

#[cfg(unix)]
fn login_shell_env() -> Option<HashMap<String, String>> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());
    let script = format!(r"printf '\n{BEGIN}\n'; env; printf '\n{END}\n'");

    // Run on a helper thread so a hung shell rc file cannot block app startup.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let output = Command::new(shell)
            .args(["-l", "-i", "-c", &script])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(output);
    });
    let output = rx.recv_timeout(Duration::from_secs(8)).ok()?.ok()?;
    parse_env_block(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(windows)]
fn login_shell_env() -> Option<HashMap<String, String>> {
    // Windows GUI processes already inherit the full user environment.
    None
}

#[cfg(unix)]
fn fallback_dirs(env: &HashMap<String, String>) -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
    ];
    if let Some(home) = env.get("HOME") {
        for rel in [".cargo/bin", ".local/bin", ".volta/bin", ".npm-global/bin"] {
            dirs.push(Path::new(home).join(rel));
        }
    }
    dirs
}

#[cfg(windows)]
fn fallback_dirs(env: &HashMap<String, String>) -> Vec<PathBuf> {
    env.get("APPDATA")
        .map(|appdata| vec![Path::new(appdata).join("npm")])
        .unwrap_or_default()
}

/// Extracts the `env` output printed between our sentinel markers. Values may
/// span multiple lines, so continuation lines are appended to the last key.
#[cfg(any(unix, test))]
fn parse_env_block(text: &str) -> Option<HashMap<String, String>> {
    let mut lines = text.lines();
    lines.by_ref().find(|line| *line == BEGIN)?;

    let mut env = HashMap::new();
    let mut last_key: Option<String> = None;
    for line in lines {
        if line == END {
            break;
        }
        match line.split_once('=') {
            Some((key, value)) if is_env_key(key) => {
                env.insert(key.to_string(), value.to_string());
                last_key = Some(key.to_string());
            }
            _ => {
                if let Some(key) = &last_key {
                    if let Some(value) = env.get_mut(key) {
                        value.push('\n');
                        value.push_str(line);
                    }
                }
            }
        }
    }
    if env.is_empty() {
        None
    } else {
        Some(env)
    }
}

#[cfg(any(unix, test))]
fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_block_between_markers() {
        let raw = format!("noise from rc\n{BEGIN}\nPATH=/a:/b\nEDITOR=vim\nMULTI=line1\nline2\n{END}\ntrailing");
        let env = parse_env_block(&raw).expect("block should parse");
        assert_eq!(env.get("PATH").map(String::as_str), Some("/a:/b"));
        assert_eq!(env.get("EDITOR").map(String::as_str), Some("vim"));
        assert_eq!(env.get("MULTI").map(String::as_str), Some("line1\nline2"));
        assert!(!env.contains_key("trailing"));
    }

    #[test]
    fn returns_none_without_markers() {
        assert!(parse_env_block("PATH=/bin").is_none());
    }
}
