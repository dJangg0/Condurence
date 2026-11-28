//! Archiving a temporary workspace, end to end on local git: a real bare `origin`, real worktrees,
//! and a stand-in for the `gh` CLI that answers `pr view` the way GitHub does. Nothing here
//! archives by itself: every call is the user's own request.

// The gh stand-in is a shell script.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use condurence_lib::git::locks::RepoLocks;
use condurence_lib::git::worktree;
use condurence_lib::workspaces::{archive_preview, archive_workspace, ArchiveOutcome};

const BRANCH: &str = "ahri";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    !git(repo, &["branch", "--list", branch]).is_empty()
}

/// A `gh` that answers `pr view` from `pull.json` and says there is no PR while the file is absent.
fn fake_gh(dir: &Path) -> PathBuf {
    let pull = dir.join("pull.json");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  if [ -f "{pull}" ]; then cat "{pull}"; exit 0; fi
  echo "no pull requests found for branch" >&2
  exit 1
fi
echo "unexpected gh call: $@" >&2
exit 2
"#,
        pull = pull.display()
    );
    let path = dir.join("gh");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// What GitHub says about the branch's PR: its state and the commit at its tip.
fn set_pr(dir: &Path, state: &str, head_sha: &str) {
    std::fs::write(
        dir.join("pull.json"),
        format!(
            r#"{{"number":7,"url":"https://github.com/acme/demo/pull/7","state":"{state}","isDraft":false,"mergeable":"UNKNOWN","mergeStateStatus":"UNKNOWN","reviewDecision":"","statusCheckRollup":[],"headRefOid":"{head_sha}"}}"#
        ),
    )
    .unwrap();
}

struct Setup {
    /// Keeps the folders alive.
    dir: tempfile::TempDir,
    repo: PathBuf,
    workspace: PathBuf,
    locks: RepoLocks,
}

/// A clone of a bare `origin` holding `main`, with one temporary workspace `ahri` started from it.
async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
    let repo = dir.path().join("demo");
    git(dir.path(), &["clone", "-q", origin.to_str().unwrap(), repo.to_str().unwrap()]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Condurence Test"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(&repo, &["push", "-q", "origin", "main"]);

    let locks = RepoLocks::default();
    let info = worktree::create_worktree(&locks, &repo, BRANCH, BRANCH, Some("origin/main")).await.unwrap();
    Setup { dir, repo, workspace: PathBuf::from(info.path), locks }
}

impl Setup {
    fn commit_in_workspace(&self, file: &str) -> String {
        std::fs::write(self.workspace.join(file), "work\n").unwrap();
        git(&self.workspace, &["add", "."]);
        git(&self.workspace, &["commit", "-q", "-m", file]);
        git(&self.workspace, &["rev-parse", "HEAD"])
    }

    async fn archive(&self, gh: Option<&Path>, delete_branch: bool, discard: bool) -> condurence_lib::error::AppResult<ArchiveOutcome> {
        archive_workspace(&self.locks, gh, &self.repo, Some(BRANCH), BRANCH, "origin", delete_branch, discard).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn uncommitted_work_is_listed_and_archiving_refuses_to_discard_it_unasked() {
    let s = setup().await;
    std::fs::write(s.workspace.join("README.md"), "# demo, edited\n").unwrap(); // tracked edit
    std::fs::create_dir_all(s.workspace.join("notes")).unwrap();
    std::fs::write(s.workspace.join("notes").join("todo.md"), "- ship\n").unwrap(); // untracked

    let preview = archive_preview(None, &s.workspace, Some(BRANCH), "origin").await.unwrap();
    let mut files = preview.uncommitted.clone();
    files.sort();
    assert_eq!(files, ["README.md", "notes/todo.md"]);

    let error = s.archive(None, false, false).await.expect_err("uncommitted changes need an explicit yes");
    assert!(error.to_string().contains("2 uncommitted files"), "{error}");
    assert!(s.workspace.exists(), "nothing was removed");
    assert_eq!(std::fs::read_to_string(s.workspace.join("README.md")).unwrap(), "# demo, edited\n");

    s.archive(None, false, true).await.expect("archive with the changes discarded");
    assert!(!s.workspace.exists());
    assert!(branch_exists(&s.repo, BRANCH), "the branch stays when the setting is off");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clean_workspace_is_archived_and_a_merged_branch_is_deleted() {
    let s = setup().await;
    s.commit_in_workspace("feature.txt");
    // Merged the ordinary way and landed on origin, so git itself sees the branch as merged. The
    // workspace branch tracks origin/main, and `git branch -d` checks against that upstream.
    git(&s.repo, &["merge", "-q", "--no-ff", "-m", "merge ahri", BRANCH]);
    git(&s.repo, &["push", "-q", "origin", "main"]);

    let outcome = s.archive(None, true, false).await.unwrap();
    assert!(!s.workspace.exists());
    assert_eq!(outcome, ArchiveOutcome { branch_deleted: true, branch_note: None });
    assert!(!branch_exists(&s.repo, BRANCH));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_branch_stays_when_the_setting_asks_to_keep_it() {
    let s = setup().await;
    s.commit_in_workspace("feature.txt");
    let outcome = s.archive(None, false, false).await.unwrap();
    assert_eq!(outcome, ArchiveOutcome { branch_deleted: false, branch_note: None });
    assert!(branch_exists(&s.repo, BRANCH));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_squash_merged_branch_is_deleted_because_github_says_its_pr_is_merged() {
    let s = setup().await;
    let tip = s.commit_in_workspace("feature.txt");
    git(&s.workspace, &["push", "-q", "origin", BRANCH]);
    // GitHub's squash merge: main gets the same content in one new commit, so the branch is not
    // an ancestor of main and git's own `branch -d` refuses it.
    git(&s.repo, &["merge", "-q", "--squash", BRANCH]);
    git(&s.repo, &["commit", "-q", "-m", "squash ahri"]);
    let gh = fake_gh(s.dir.path());
    set_pr(s.dir.path(), "MERGED", &tip);

    let preview = archive_preview(Some(&gh), &s.workspace, Some(BRANCH), "origin").await.unwrap();
    assert_eq!(preview.pr.as_ref().map(|pr| pr.state.as_str()), Some("MERGED"));
    assert!(preview.delivered, "the merged PR's last commit is the branch tip");

    let outcome = s.archive(Some(&gh), true, false).await.unwrap();
    assert_eq!(outcome, ArchiveOutcome { branch_deleted: true, branch_note: None });
    assert!(!s.workspace.exists());
    assert!(!branch_exists(&s.repo, BRANCH), "the merged branch is gone, though git cannot see it as merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_that_moved_on_after_its_pr_merged_is_kept_with_its_new_commit() {
    let s = setup().await;
    let merged_tip = s.commit_in_workspace("feature.txt");
    let gh = fake_gh(s.dir.path());
    set_pr(s.dir.path(), "MERGED", &merged_tip);
    let newer = s.commit_in_workspace("after-the-merge.txt");

    let preview = archive_preview(Some(&gh), &s.workspace, Some(BRANCH), "origin").await.unwrap();
    assert!(!preview.delivered, "the branch has a commit the merged PR does not");

    let outcome = s.archive(Some(&gh), true, false).await.unwrap();
    assert!(!s.workspace.exists());
    assert!(!outcome.branch_deleted);
    assert!(outcome.branch_note.as_deref().is_some_and(|note| note.contains("not merged")), "{outcome:?}");
    assert_eq!(git(&s.repo, &["rev-parse", BRANCH]), newer, "the commit made after the merge is still on the branch");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unmerged_branch_is_kept_with_its_commits_and_the_reason_is_given() {
    let s = setup().await;
    let tip = s.commit_in_workspace("unmerged.txt");

    let outcome = s.archive(None, true, false).await.unwrap();
    assert!(!outcome.branch_deleted);
    assert!(outcome.branch_note.as_deref().is_some_and(|note| note.contains("not merged")), "{outcome:?}");
    assert_eq!(git(&s.repo, &["rev-parse", BRANCH]), tip);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_preview_counts_commits_the_remote_does_not_have() {
    let s = setup().await;
    s.commit_in_workspace("one.txt");
    s.commit_in_workspace("two.txt");
    let before = archive_preview(None, &s.workspace, Some(BRANCH), "origin").await.unwrap();
    assert_eq!((before.unpushed, before.uncommitted.len(), before.pr, before.delivered), (2, 0, None, false));

    git(&s.workspace, &["push", "-q", "origin", BRANCH]);
    let after = archive_preview(None, &s.workspace, Some(BRANCH), "origin").await.unwrap();
    assert_eq!(after.unpushed, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_without_a_pr_has_none_and_is_not_delivered() {
    let s = setup().await;
    let gh = fake_gh(s.dir.path()); // no pull.json: "no pull requests found"
    let preview = archive_preview(Some(&gh), &s.workspace, Some(BRANCH), "origin").await.unwrap();
    assert_eq!((preview.pr, preview.delivered), (None, false));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_detached_workspace_is_archived_without_a_branch() {
    let s = setup().await;
    git(&s.repo, &["worktree", "add", "-q", "--detach", ".worktrees/scratch", "HEAD"]);
    let scratch = s.repo.join(".worktrees").join("scratch");
    assert!(scratch.exists());

    let outcome = archive_workspace(&s.locks, None, &s.repo, None, "scratch", "origin", true, false).await.unwrap();
    assert_eq!(outcome, ArchiveOutcome { branch_deleted: false, branch_note: None });
    assert!(!scratch.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_workspaces_of_the_repository_can_be_archived() {
    let s = setup().await;
    for folder in ["", "../demo", "a/b", ".hidden", "no-such-workspace"] {
        let error = archive_workspace(&s.locks, None, &s.repo, Some(BRANCH), folder, "origin", false, true)
            .await
            .expect_err(folder);
        assert!(!error.to_string().is_empty());
    }
    assert!(s.repo.join("README.md").exists(), "the main checkout is untouched");
    assert!(s.workspace.exists());
    assert!(
        archive_workspace(&s.locks, None, &s.repo, Some("-bad"), BRANCH, "origin", false, true).await.is_err(),
        "a branch name that git could read as a flag is refused"
    );
    assert!(s.workspace.exists());
}
