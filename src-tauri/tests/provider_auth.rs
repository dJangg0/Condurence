//! Provider sign-in status against fake CLIs that print what the real ones print.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use condurence_lib::auth;
use condurence_lib::graph::AgentProvider;

fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn missing_cli_is_reported_as_not_installed() {
    let status = auth::provider_status(AgentProvider::Codex, None).await;
    assert!(!status.installed && !status.logged_in);
    assert_eq!(status.provider, "codex");
}

#[tokio::test]
async fn signed_in_and_signed_out_clis_are_told_apart() {
    let tmp = tempfile::tempdir().unwrap();
    let claude_in = fake(tmp.path(), "claude_in", r#"echo '{"loggedIn":true,"email":"a@b.c","subscriptionType":"max"}'"#);
    let claude_out = fake(tmp.path(), "claude_out", r#"echo '{"loggedIn":false}'; exit 1"#);
    let codex_in = fake(tmp.path(), "codex_in", "echo 'Logged in using ChatGPT'");
    let codex_out = fake(tmp.path(), "codex_out", "echo 'Not logged in' >&2; exit 1");
    let oc_in = fake(tmp.path(), "oc_in", "echo '└  3 credentials'");
    let oc_out = fake(tmp.path(), "oc_out", "echo '└  0 credentials'");

    let s = auth::provider_status(AgentProvider::Claude, Some(&claude_in)).await;
    assert!(s.installed && s.logged_in);
    assert_eq!(s.detail.as_deref(), Some("a@b.c · max"));
    assert!(!auth::provider_status(AgentProvider::Claude, Some(&claude_out)).await.logged_in);

    let s = auth::provider_status(AgentProvider::Codex, Some(&codex_in)).await;
    assert!(s.logged_in);
    assert_eq!(s.detail.as_deref(), Some("ChatGPT"));
    let s = auth::provider_status(AgentProvider::Codex, Some(&codex_out)).await;
    assert!(s.installed && !s.logged_in);

    let s = auth::provider_status(AgentProvider::Opencode, Some(&oc_in)).await;
    assert!(s.logged_in);
    assert_eq!(s.detail.as_deref(), Some("3 providers"));
    assert!(!auth::provider_status(AgentProvider::Opencode, Some(&oc_out)).await.logged_in);
}

#[tokio::test]
async fn a_cli_that_prints_nothing_useful_is_treated_as_signed_out() {
    let tmp = tempfile::tempdir().unwrap();
    let broken = fake(tmp.path(), "broken", "echo 'boom' >&2; exit 3");
    let s = auth::provider_status(AgentProvider::Claude, Some(&broken)).await;
    assert!(s.installed && !s.logged_in);
}
