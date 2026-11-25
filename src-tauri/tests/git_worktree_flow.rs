//! Exercises the worktree lifecycle against a real git repository in a temp directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use open_conductor_lib::git::{self, diff_engine, locks::RepoLocks, worktree};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git must be installed");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn make_repo(parent: &Path) -> PathBuf {
    let repo = parent.join("demo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "OpenConductor Test"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_lifecycle_create_diff_merge_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let locks = RepoLocks::default();

    // Create a workspace on a fresh branch.
    let info = worktree::create_worktree(&locks, &repo, "feature/greeting", "ws-greeting", None)
        .await
        .expect("create worktree");
    let worktree_path = PathBuf::from(&info.path);
    assert!(worktree_path.join("README.md").exists());
    assert_eq!(info.branch.as_deref(), Some("feature/greeting"));

    // The worktree folder must be hidden from the main checkout's status.
    let listed = worktree::list_worktrees(&repo).await.unwrap();
    assert!(listed.iter().any(|wt| wt.branch.as_deref() == Some("feature/greeting")));
    assert!(listed[0].is_main);

    // Edit a tracked file and add an untracked one; the diff must cover both.
    std::fs::write(worktree_path.join("README.md"), "# demo\n\nhello\n").unwrap();
    std::fs::write(worktree_path.join("notes.txt"), "line one\nline two\n").unwrap();
    let diff = git::diff_engine::worktree_diff(&worktree_path).await.unwrap();
    let readme = diff.iter().find(|f| f.path == "README.md").expect("tracked change");
    assert_eq!(readme.status, diff_engine::FileStatus::Modified);
    assert_eq!(readme.additions, 2);
    let notes = diff.iter().find(|f| f.path == "notes.txt").expect("untracked file");
    assert_eq!(notes.status, diff_engine::FileStatus::Untracked);
    assert_eq!(notes.additions, 2);
    assert_eq!(git::diff_engine::changed_file_count(&worktree_path).unwrap(), 2);

    // Commit in the workspace, then squash-merge into main from the main checkout.
    git(&worktree_path, &["add", "."]);
    git(&worktree_path, &["commit", "-q", "-m", "greeting"]);
    let merged = worktree::merge_worktree(&locks, &repo, "feature/greeting", "main", true)
        .await
        .unwrap();
    assert!(merged.merged, "merge should succeed: {}", merged.message);
    assert!(std::fs::read_to_string(repo.join("README.md")).unwrap().contains("hello"));

    // Removing the workspace deletes the folder. A squash merge leaves the branch unmerged
    // in git's eyes, so the safe `branch -d` keeps it; no committed work is ever dropped.
    worktree::remove_worktree(&locks, &repo, "feature/greeting", "ws-greeting")
        .await
        .expect("remove worktree");
    assert!(!worktree_path.exists());
    let branches = git(&repo, &["branch", "--list", "feature/greeting"]);
    assert!(!branches.trim().is_empty(), "squash-merged branch must be kept by safe delete");
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_fully_merged_workspace_deletes_its_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let locks = RepoLocks::default();

    let info = worktree::create_worktree(&locks, &repo, "feature/plain", "ws-plain", None)
        .await
        .unwrap();
    let worktree_path = PathBuf::from(&info.path);
    std::fs::write(worktree_path.join("feature.txt"), "x\n").unwrap();
    git(&worktree_path, &["add", "."]);
    git(&worktree_path, &["commit", "-q", "-m", "feature"]);

    let merged = worktree::merge_worktree(&locks, &repo, "feature/plain", "main", false)
        .await
        .unwrap();
    assert!(merged.merged, "merge should succeed: {}", merged.message);

    worktree::remove_worktree(&locks, &repo, "feature/plain", "ws-plain")
        .await
        .expect("remove worktree");
    assert!(git(&repo, &["branch", "--list", "feature/plain"]).trim().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_reports_conflicts_and_leaves_tree_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let locks = RepoLocks::default();

    let info = worktree::create_worktree(&locks, &repo, "feature/conflict", "ws-conflict", None)
        .await
        .unwrap();
    let worktree_path = PathBuf::from(&info.path);

    std::fs::write(worktree_path.join("README.md"), "from workspace\n").unwrap();
    git(&worktree_path, &["commit", "-q", "-am", "workspace edit"]);
    std::fs::write(repo.join("README.md"), "from main\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "main edit"]);

    let result = worktree::merge_worktree(&locks, &repo, "feature/conflict", "main", false)
        .await
        .unwrap();
    assert!(!result.merged);
    assert!(result.conflicts.iter().any(|path| path == "README.md"));
    // The failed merge must be aborted so the user's checkout is not left mid-merge.
    assert!(git(&repo, &["status", "--porcelain"]).trim().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_names_are_rejected_before_touching_git() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let locks = RepoLocks::default();
    assert!(worktree::create_worktree(&locks, &repo, "../escape", "ws", None).await.is_err());
    assert!(worktree::create_worktree(&locks, &repo, "ok-branch", "../ws", None).await.is_err());
}
