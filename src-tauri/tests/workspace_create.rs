//! Conductor-style workspaces on real git: a bare origin, a clone with local work the origin does
//! not have, and workspaces created from the settings. Nothing leaves this machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use condurence_lib::git::locks::RepoLocks;
use condurence_lib::git::remotes;
use condurence_lib::settings::RepoGitSettings;
use condurence_lib::workspaces::{create_workspace, CHAMPIONS};

fn run_git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git").current_dir(dir).args(args).output().unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = run_git(dir, args);
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A bare `origin` with `main`, and a clone whose checkout is a local branch the origin lacks, so
/// the checkout is ahead of `origin/main` and a workspace must not start from it.
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

    git(&work, &["checkout", "-q", "-b", "local-only"]);
    std::fs::write(work.join("local.txt"), "not pushed\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "local work"]);
    (origin, work)
}

#[tokio::test]
async fn a_workspace_starts_from_the_base_with_a_champion_branch() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let locks = RepoLocks::default();
    let settings = RepoGitSettings::default();

    let first = create_workspace(&locks, &work, &settings, 0).await.expect("workspace");
    assert_eq!(first.branch.as_deref(), Some(CHAMPIONS[0]), "the first free champion, from seed 0");
    assert!(first.path.ends_with(&format!(".worktrees/{}", CHAMPIONS[0])), "{}", first.path);

    // The workspace starts from origin/main, not from the local branch that is ahead of it.
    let base = git(&work, &["rev-parse", "refs/remotes/origin/main"]);
    let head = git(Path::new(&first.path), &["rev-parse", "HEAD"]);
    assert_eq!(head, base);
    assert!(!Path::new(&first.path).join("local.txt").exists(), "the unpushed local file must not come along");

    // Upstream on plain push is on by default, and it is written to this workspace only.
    assert_eq!(git(Path::new(&first.path), &["config", "--worktree", "--get", "push.autoSetupRemote"]), "true");
}

#[tokio::test]
async fn the_next_workspace_takes_the_next_free_champion() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let locks = RepoLocks::default();
    let settings = RepoGitSettings::default();

    create_workspace(&locks, &work, &settings, 0).await.unwrap();
    let second = create_workspace(&locks, &work, &settings, 0).await.unwrap();
    assert_eq!(second.branch.as_deref(), Some(CHAMPIONS[1]));
}

#[tokio::test]
async fn turning_upstream_off_leaves_the_workspace_config_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let locks = RepoLocks::default();
    let settings = RepoGitSettings { set_upstream_on_push: false, ..RepoGitSettings::default() };

    let info = create_workspace(&locks, &work, &settings, 0).await.unwrap();
    let probe = run_git(Path::new(&info.path), &["config", "--worktree", "--get", "push.autoSetupRemote"]);
    assert!(!probe.status.success(), "no upstream setting should be written");
}

#[tokio::test]
async fn a_workspace_can_start_from_another_remote_branch_such_as_beta() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = setup(dir.path());
    git(&work, &["push", "-q", "origin", "main:beta"]);
    git(&work, &["fetch", "-q", "origin"]);
    let locks = RepoLocks::default();
    let settings = RepoGitSettings { base_ref: "origin/beta".into(), ..RepoGitSettings::default() };

    let info = create_workspace(&locks, &work, &settings, 0).await.unwrap();
    assert_eq!(git(Path::new(&info.path), &["rev-parse", "HEAD"]), git(&origin, &["rev-parse", "refs/heads/beta"]));
}

#[tokio::test]
async fn a_missing_base_is_explained_rather_than_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    let locks = RepoLocks::default();
    let settings = RepoGitSettings { base_ref: "origin/develop".into(), ..RepoGitSettings::default() };

    let error = create_workspace(&locks, &work, &settings, 0).await.expect_err("no develop branch exists");
    assert!(error.to_string().contains("origin/develop is not available"), "{error}");
    assert!(error.to_string().contains("repository settings"), "{error}");
}

#[tokio::test]
async fn the_remote_lists_show_branches_without_the_remote_head() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, work) = setup(dir.path());
    git(&work, &["push", "-q", "origin", "main:beta"]);

    assert_eq!(remotes::list_remotes(&work).await.unwrap(), ["origin"]);
    let branches = remotes::list_remote_branches(&work, "origin").await.unwrap();
    assert!(branches.contains(&"origin/main".to_string()), "{branches:?}");
    assert!(branches.contains(&"origin/beta".to_string()), "{branches:?}");
    assert!(!branches.iter().any(|branch| branch.ends_with("/HEAD")), "{branches:?}");
}
