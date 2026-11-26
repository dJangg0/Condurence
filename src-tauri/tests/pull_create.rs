//! Opening a PR from a workspace branch, end to end on local git: a real push to a bare remote,
//! with a stand-in for the `gh` CLI that records its arguments and answers the way GitHub does.
//! Nothing is published, so this can run anywhere.

// Uses a shell script as the gh stand-in, so it is Unix-only.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use open_conductor_lib::pulls;

const BRANCH: &str = "feature/greeting";

const PULL_JSON: &str = r#"{"number":7,"url":"https://github.com/acme/demo/pull/7","state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":"","statusCheckRollup":[]}"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A `gh` that keeps a PR open after `pr create`, reports it through `pr view`, refuses a second
/// create, and logs every call.
fn fake_gh(dir: &Path) -> PathBuf {
    let state = dir.join("pr-open");
    let log = dir.join("gh.log");
    let pull = dir.join("pull.json");
    std::fs::write(&pull, PULL_JSON).unwrap();
    let script = format!(
        r#"#!/bin/sh
echo "$@" >> "{log}"
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  if [ -f "{state}" ]; then echo "a pull request for branch already exists" >&2; exit 1; fi
  touch "{state}"
  echo "https://github.com/acme/demo/pull/7"
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  if [ -f "{state}" ]; then cat "{pull}"; exit 0; fi
  echo "no pull requests found for branch" >&2
  exit 1
fi
echo "unexpected gh call: $@" >&2
exit 2
"#,
        log = log.display(),
        state = state.display(),
        pull = pull.display(),
    );
    let path = dir.join("gh");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A bare `origin` with `main`, and a clone whose `feature/greeting` branch has one commit.
fn setup(parent: &Path) -> (PathBuf, PathBuf) {
    let origin = parent.join("origin.git");
    git(parent, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
    let work = parent.join("demo");
    git(parent, &["clone", "-q", origin.to_str().unwrap(), work.to_str().unwrap()]);
    git(&work, &["config", "user.email", "test@example.com"]);
    git(&work, &["config", "user.name", "Condurence Test"]);

    std::fs::write(work.join("README.md"), "# demo\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "init"]);
    git(&work, &["push", "-q", "origin", "main"]);

    git(&work, &["checkout", "-q", "-b", BRANCH]);
    std::fs::write(work.join("greeting.txt"), "hello\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "add greeting"]);
    (origin, work)
}

#[tokio::test]
async fn a_pr_opens_from_the_committed_branch() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    let gh = fake_gh(dir.path());

    // Before creating, GitHub has no PR for the branch.
    assert!(pulls::status(Some(&gh), &work, BRANCH).await.unwrap().is_none());

    let pr = pulls::create(Some(&gh), &work, BRANCH, "main").await.expect("create should open the PR");
    assert_eq!(pr.number, 7);
    assert_eq!(pr.state, "OPEN");
    assert_eq!(pr.url, "https://github.com/acme/demo/pull/7");

    // The branch reached origin with its commit, and the worktree tracks it.
    let head = git(&work, &["rev-parse", "HEAD"]);
    assert_eq!(git(&origin, &["rev-parse", &format!("refs/heads/{BRANCH}")]), head);
    assert_eq!(git(&work, &["rev-parse", "--abbrev-ref", "@{u}"]), format!("origin/{BRANCH}"));

    // gh was asked for a PR into main from this branch, titled and described from its commits.
    let log = std::fs::read_to_string(dir.path().join("gh.log")).unwrap();
    assert!(log.contains("pr create --base main --head feature/greeting --fill"), "gh was called with:\n{log}");

    // Once it exists, the workspace sees the PR and the status poll reads it back.
    let seen = pulls::status(Some(&gh), &work, BRANCH).await.unwrap().expect("the PR should now be visible");
    assert_eq!(seen.number, 7);
}

#[tokio::test]
async fn a_second_create_surfaces_gh_s_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let gh = fake_gh(dir.path());
    pulls::create(Some(&gh), &work, BRANCH, "main").await.expect("first create");

    let error = pulls::create(Some(&gh), &work, BRANCH, "main").await.expect_err("a duplicate PR must be refused");
    assert!(error.to_string().contains("already exists"), "unexpected error: {error}");
}

#[tokio::test]
async fn create_needs_the_gh_cli() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let error = pulls::create(None, &work, BRANCH, "main").await.expect_err("no gh, no PR");
    assert!(error.to_string().contains("GitHub CLI"), "unexpected error: {error}");
}

#[tokio::test]
async fn uncommitted_edits_stay_out_of_the_pr() {
    // Current behaviour: create pushes what is committed. Edits that are not committed yet are
    // left out, so the PR can be missing work the user expects it to have.
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    let gh = fake_gh(dir.path());
    std::fs::write(work.join("unsaved.txt"), "not committed\n").unwrap();

    pulls::create(Some(&gh), &work, BRANCH, "main").await.expect("create");

    let remote_files = git(&origin, &["ls-tree", "--name-only", &format!("refs/heads/{BRANCH}")]);
    assert!(remote_files.contains("greeting.txt"));
    assert!(!remote_files.contains("unsaved.txt"), "uncommitted work must not be pushed: {remote_files}");
}
