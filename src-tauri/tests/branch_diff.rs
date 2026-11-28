//! What a branch would put in a PR: its commits since the base. Uncommitted edits are left out, so
//! the PR view never shows work that is not part of the branch yet.

use std::path::Path;
use std::process::Command;

use condurence_lib::git::diff_engine::branch_diff;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

#[tokio::test]
async fn the_branch_diff_is_its_commits_and_not_the_uncommitted_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Condurence Test"]);
    std::fs::write(repo.join("README.md"), "# demo\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "init"]);

    git(repo, &["checkout", "-q", "-b", "feature/greeting"]);
    std::fs::write(repo.join("greeting.txt"), "hello\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "add greeting"]);

    // Uncommitted: an edit to a tracked file and a new untracked file.
    std::fs::write(repo.join("README.md"), "# demo, edited\n").unwrap();
    std::fs::write(repo.join("scratch.txt"), "not yet\n").unwrap();

    let files = branch_diff(repo, "main").await.expect("diff against main");
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["greeting.txt"], "only the committed change belongs to the PR");
    assert!(files[0].additions >= 1);

    assert!(branch_diff(repo, "origin/main").await.is_err(), "a base that does not exist is an error");
}
