//! Opening a PR from a workspace branch, end to end on local git: a real push to a bare remote,
//! with a stand-in for the `gh` CLI that records its arguments and answers the way GitHub does.
//! Nothing is published, so this can run anywhere.

// Uses a shell script as the gh stand-in, so it is Unix-only.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use condurence_lib::pulls;

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
    assert!(pulls::status(Some(&gh), &work, BRANCH, "origin").await.unwrap().is_none());

    let pr = pulls::create(Some(&gh), &work, BRANCH, "main", "origin").await.expect("create should open the PR");
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
    let seen = pulls::status(Some(&gh), &work, BRANCH, "origin").await.unwrap().expect("the PR should now be visible");
    assert_eq!(seen.number, 7);
}

#[tokio::test]
async fn a_second_create_surfaces_gh_s_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let gh = fake_gh(dir.path());
    pulls::create(Some(&gh), &work, BRANCH, "main", "origin").await.expect("first create");

    let error = pulls::create(Some(&gh), &work, BRANCH, "main", "origin").await.expect_err("a duplicate PR must be refused");
    assert!(error.to_string().contains("already exists"), "unexpected error: {error}");
}

#[tokio::test]
async fn a_branch_with_no_commits_is_not_pushed_or_opened() {
    // A workspace whose branch has no commits yet (only edits, or nothing) has no PR to open. It must
    // fail with a plain message before anything is pushed or gh is called.
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    let gh = fake_gh(dir.path());
    let empty = "feature/empty";
    git(&work, &["checkout", "-q", "-b", empty, "main"]);

    let error = pulls::create(Some(&gh), &work, empty, "main", "origin").await.expect_err("nothing to open a PR for");
    assert!(error.to_string().contains("no commits beyond origin/main"), "unexpected error: {error}");
    assert!(error.to_string().contains("Commit your changes first"), "unexpected error: {error}");

    let remote_refs = git(&origin, &["for-each-ref", "--format=%(refname)"]);
    assert!(!remote_refs.contains(empty), "an empty branch must not be pushed: {remote_refs}");
    assert!(!dir.path().join("gh.log").exists(), "gh must not be called for an empty branch");
}

#[tokio::test]
async fn create_needs_the_gh_cli() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let error = pulls::create(None, &work, BRANCH, "main", "origin").await.expect_err("no gh, no PR");
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

    pulls::create(Some(&gh), &work, BRANCH, "main", "origin").await.expect("create");

    let remote_files = git(&origin, &["ls-tree", "--name-only", &format!("refs/heads/{BRANCH}")]);
    assert!(remote_files.contains("greeting.txt"));
    assert!(!remote_files.contains("unsaved.txt"), "uncommitted work must not be pushed: {remote_files}");
}

#[tokio::test]
async fn a_manual_commit_puts_the_pending_edits_into_the_pr() {
    // The explicit flow: commit what the agent left behind, then open the PR. The edit reaches
    // the remote, which the plain create above cannot do on its own.
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    let gh = fake_gh(dir.path());
    std::fs::write(work.join("unsaved.txt"), "agent edit\n").unwrap();

    let commit = pulls::commit_changes(&work, "Agent changes: add a greeting").await.unwrap();
    assert!(commit.is_some(), "a dirty worktree must produce a commit");
    assert_eq!(git(&work, &["status", "--porcelain"]), "", "the worktree is clean after the commit");
    assert_eq!(
        pulls::commit_changes(&work, "again").await.unwrap(),
        None,
        "committing a clean worktree is a no-op"
    );
    assert!(pulls::commit_changes(&work, "   ").await.is_err(), "an empty message is refused");

    pulls::create(Some(&gh), &work, BRANCH, "main", "origin").await.expect("create should open the PR");
    let remote_files = git(&origin, &["ls-tree", "--name-only", &format!("refs/heads/{BRANCH}")]);
    assert!(remote_files.contains("unsaved.txt"), "the committed edit must be on the remote: {remote_files}");
}

#[tokio::test]
async fn work_on_the_base_branch_moves_to_its_own_branch_and_opens_a_pr() {
    // The root agent and shared agents work in the main checkout, on `main`. A PR cannot start from
    // the base branch, so that work is moved onto a new branch first: the commit made there and the
    // edit still uncommitted both come along, and `main` itself is left where it was.
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    let gh = fake_gh(dir.path());
    git(&work, &["checkout", "-q", "main"]);
    std::fs::write(work.join("agent.txt"), "committed on main\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "agent work on main"]);
    let main_head = git(&work, &["rev-parse", "main"]);
    std::fs::write(work.join("unsaved.txt"), "still uncommitted\n").unwrap();

    let branch = "oc/abc12345/changes";
    pulls::start_branch(&work, branch).await.expect("start the branch");
    assert_eq!(git(&work, &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
    assert_eq!(git(&work, &["rev-parse", "main"]), main_head, "main is not moved");
    assert!(git(&work, &["status", "--porcelain"]).contains("unsaved.txt"), "the uncommitted edit came along");

    pulls::commit_changes(&work, "Agent changes: work on main").await.expect("commit");
    let pr = pulls::create(Some(&gh), &work, branch, "main", "origin").await.expect("open the PR");
    assert_eq!(pr.number, 7);

    let remote_files = git(&origin, &["ls-tree", "--name-only", &format!("refs/heads/{branch}")]);
    assert!(remote_files.contains("agent.txt") && remote_files.contains("unsaved.txt"), "{remote_files}");
    assert_ne!(git(&origin, &["rev-parse", "refs/heads/main"]), main_head, "the remote main is untouched");
    let log = std::fs::read_to_string(dir.path().join("gh.log")).unwrap();
    assert!(log.contains("pr create --base main --head oc/abc12345/changes --fill"), "gh was called with:\n{log}");
}

#[tokio::test]
async fn a_new_branch_needs_a_valid_name_that_is_not_taken() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    for bad in ["", "-flag", "has space", "a..b", "colon:name"] {
        assert!(pulls::start_branch(&work, bad).await.is_err(), "{bad:?} should be refused");
    }
    // `main` already exists.
    let error = pulls::start_branch(&work, "main").await.expect_err("a taken name");
    assert!(error.to_string().contains("main"), "{error}");
    assert_eq!(git(&work, &["rev-parse", "--abbrev-ref", "HEAD"]), BRANCH, "a refused start leaves the branch alone");
}
