//! `pulls::sync_with_base` against real git repositories, with a local bare repository as `origin`.
//! No network and no `gh`: only the git half of the PR flow runs here.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use condurence_lib::pulls;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").current_dir(dir).args(args).output().expect("git must be installed");
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn commit_file(repo: &Path, name: &str, body: &str, message: &str) {
    std::fs::write(repo.join(name), body).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", message]);
}

/// A clone with `main` pushed to a bare origin, plus a `feature` branch checked out.
fn setup(parent: &Path) -> (PathBuf, PathBuf) {
    let origin = parent.join("origin.git");
    git(parent, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
    let work = parent.join("work");
    git(parent, &["clone", "-q", origin.to_str().unwrap(), work.to_str().unwrap()]);
    git(&work, &["config", "user.email", "test@example.com"]);
    git(&work, &["config", "user.name", "Condurence Test"]);
    git(&work, &["checkout", "-q", "-b", "main"]);
    commit_file(&work, "shared.txt", "one\n", "init");
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["checkout", "-q", "-b", "feature"]);
    (origin, work)
}

/// Lands a commit on the origin's `main` through a second clone.
fn advance_main(parent: &Path, origin: &Path, file: &str, body: &str) {
    let other = parent.join("other");
    if !other.exists() {
        git(parent, &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
        git(&other, &["config", "user.email", "test@example.com"]);
        git(&other, &["config", "user.name", "Condurence Test"]);
    }
    git(&other, &["checkout", "-q", "main"]);
    git(&other, &["pull", "-q", "origin", "main"]);
    commit_file(&other, file, body, "advance main");
    git(&other, &["push", "-q", "origin", "main"]);
}

#[tokio::test]
async fn a_clean_merge_is_pushed() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    commit_file(&work, "feature.txt", "f\n", "feature work");
    advance_main(dir.path(), &origin, "other.txt", "o\n");

    let outcome = pulls::sync_with_base(&work, "feature", "main", "origin").await.unwrap();
    assert!(outcome.pushed, "{outcome:?}");
    assert!(outcome.conflicts.is_empty());
    assert!(work.join("other.txt").exists(), "main's change was merged into the branch");
    let remote = git(dir.path(), &["--git-dir", origin.to_str().unwrap(), "branch", "--list", "feature"]);
    assert!(remote.contains("feature"), "the branch was pushed");
}

#[tokio::test]
async fn conflicts_are_listed_and_the_next_call_finishes_the_merge() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    commit_file(&work, "shared.txt", "feature\n", "feature edit");
    advance_main(dir.path(), &origin, "shared.txt", "main\n");

    let first = pulls::sync_with_base(&work, "feature", "main", "origin").await.unwrap();
    assert!(!first.pushed);
    assert_eq!(first.conflicts, ["shared.txt"]);

    // Still conflicted: asking again reports the same files and does not push.
    let again = pulls::sync_with_base(&work, "feature", "main", "origin").await.unwrap();
    assert!(!again.pushed);
    assert_eq!(again.conflicts, ["shared.txt"]);

    // The agent resolves and stages the file; the next press commits the merge and pushes.
    std::fs::write(work.join("shared.txt"), "both\n").unwrap();
    git(&work, &["add", "shared.txt"]);
    let done = pulls::sync_with_base(&work, "feature", "main", "origin").await.unwrap();
    assert!(done.pushed, "{done:?}");
    assert!(done.conflicts.is_empty());
    assert!(git(&work, &["status", "--porcelain"]).trim().is_empty());
}

#[tokio::test]
async fn uncommitted_work_blocks_the_sync() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    std::fs::write(work.join("scratch.txt"), "wip\n").unwrap();

    let error = pulls::sync_with_base(&work, "feature", "main", "origin").await.unwrap_err().to_string();
    assert!(error.contains("Commit or stash"), "{error}");
}

#[tokio::test]
async fn branch_names_that_read_as_options_never_reach_git() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    assert!(pulls::sync_with_base(&work, "--force", "main", "origin").await.is_err());
    assert!(pulls::sync_with_base(&work, "feature", "--upload-pack=x", "origin").await.is_err());
}
