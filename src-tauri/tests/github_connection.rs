//! Exercises the GitHub connection against a real git repository and a fake `gh` script.
//! The fake answers only the subcommands Condurence calls, so these tests never reach
//! GitHub or depend on the developer's account.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use open_conductor_lib::github::{self, LinkState};

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

/// The `origin` URL git stores for `repo`, or `None` when there is no remote.
fn origin(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .expect("git must be installed");
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Writes an executable `gh` stand-in. `signed_in` decides whether `gh auth status` succeeds.
/// Only `octo/demo` is visible to the fake account; any other repository fails the way
/// `gh repo view` does.
fn fake_gh(dir: &Path, signed_in: bool) -> PathBuf {
    let auth_exit = if signed_in { 0 } else { 1 };
    let script = format!(
        r#"#!/bin/sh
case "$1 $2" in
  "auth status")
    exit {auth_exit}
    ;;
  "api --hostname")
    echo '{{"login":"octocat","id":1,"name":"Octo Cat"}}'
    ;;
  "config get")
    echo https
    ;;
  "repo list")
    echo '[{{"nameWithOwner":"octo/demo","url":"https://github.com/octo/demo","isPrivate":false,"description":null}}]'
    ;;
  "repo clone")
    if [ "$3" = "octo/demo" ]; then
      git init -q "$4" && git -C "$4" remote add origin https://github.com/octo/demo.git
    else
      echo "Could not resolve to a Repository with the name '$3'." >&2
      exit 1
    fi
    ;;
  "repo view")
    if [ "$3" = "octo/demo" ]; then
      echo '{{"defaultBranchRef":{{"name":"main"}},"isPrivate":false,"nameWithOwner":"octo/demo","url":"https://github.com/octo/demo","viewerPermission":"ADMIN"}}'
    else
      echo "GraphQL: Could not resolve to a Repository with the name '$3'. (repository)" >&2
      exit 1
    fi
    ;;
  *)
    echo "unexpected gh call: $*" >&2
    exit 2
    ;;
esac
"#
    );
    let path = dir.join("gh");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn missing_cli_is_reported_without_running_anything() {
    let tmp = tempfile::tempdir().unwrap();
    let account = github::account_status(None, tmp.path()).await.unwrap();
    assert!(!account.cli_installed);
    assert!(!account.signed_in);
    assert_eq!(account.login, None);
}

#[tokio::test]
async fn signed_in_account_is_read_from_gh() {
    let tmp = tempfile::tempdir().unwrap();
    let gh = fake_gh(tmp.path(), true);
    let account = github::account_status(Some(&gh), tmp.path()).await.unwrap();
    assert!(account.cli_installed);
    assert!(account.signed_in);
    assert_eq!(account.login.as_deref(), Some("octocat"));
    assert_eq!(account.name.as_deref(), Some("Octo Cat"));
}

#[tokio::test]
async fn signed_out_account_is_reported_as_such() {
    let tmp = tempfile::tempdir().unwrap();
    let gh = fake_gh(tmp.path(), false);
    let account = github::account_status(Some(&gh), tmp.path()).await.unwrap();
    assert!(account.cli_installed);
    assert!(!account.signed_in);
    assert_eq!(account.login, None);
}

#[tokio::test]
async fn unlinked_repository_connects_to_its_github_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let gh = fake_gh(tmp.path(), true);

    let before = github::repo_connection(Some(&gh), &repo).await.unwrap();
    assert_eq!(before.state, LinkState::Unlinked);
    assert_eq!(origin(&repo), None);

    let linked = github::connect_repository(Some(&gh), &repo, "octo/demo").await.unwrap();
    assert_eq!(linked.state, LinkState::Linked);
    assert_eq!(linked.name_with_owner.as_deref(), Some("octo/demo"));
    assert_eq!(linked.web_url.as_deref(), Some("https://github.com/octo/demo"));
    assert_eq!(linked.default_branch.as_deref(), Some("main"));
    assert_eq!(linked.viewer_permission.as_deref(), Some("ADMIN"));
    assert_eq!(origin(&repo).as_deref(), Some("https://github.com/octo/demo.git"));
}

#[tokio::test]
async fn connecting_refuses_to_replace_an_existing_non_github_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    git(&repo, &["remote", "add", "origin", "https://gitlab.com/octo/demo.git"]);
    let gh = fake_gh(tmp.path(), true);

    let state = github::repo_connection(Some(&gh), &repo).await.unwrap();
    assert_eq!(state.state, LinkState::OtherRemote);

    let error = github::connect_repository(Some(&gh), &repo, "octo/demo").await.unwrap_err();
    assert!(
        error.to_string().contains("origin already points to https://gitlab.com/octo/demo.git"),
        "unexpected error: {error}"
    );
    assert_eq!(origin(&repo).as_deref(), Some("https://gitlab.com/octo/demo.git"));
}

#[tokio::test]
async fn github_origin_that_gh_cannot_see_is_unverified_with_the_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    git(&repo, &["remote", "add", "origin", "git@github.com:someone/private-thing.git"]);
    let gh = fake_gh(tmp.path(), true);

    let connection = github::repo_connection(Some(&gh), &repo).await.unwrap();
    assert_eq!(connection.state, LinkState::Unverified);
    assert_eq!(connection.remote_url.as_deref(), Some("git@github.com:someone/private-thing.git"));
    let detail = connection.detail.expect("unverified state explains itself");
    assert!(detail.contains("Could not resolve to a Repository"), "{detail}");
}

#[tokio::test]
async fn connecting_an_inaccessible_repository_adds_no_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let gh = fake_gh(tmp.path(), true);

    let error = github::connect_repository(Some(&gh), &repo, "someone/else").await.unwrap_err();
    assert!(error.to_string().contains("Could not resolve"), "unexpected error: {error}");
    assert_eq!(origin(&repo), None);
}

#[tokio::test]
async fn connecting_to_the_already_linked_repository_is_a_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    git(&repo, &["remote", "add", "origin", "https://github.com/octo/demo.git"]);
    let gh = fake_gh(tmp.path(), true);

    let connection = github::connect_repository(Some(&gh), &repo, "octo/demo").await.unwrap();
    assert_eq!(connection.state, LinkState::Linked);
    assert_eq!(origin(&repo).as_deref(), Some("https://github.com/octo/demo.git"));
}

#[tokio::test]
async fn invalid_repository_names_never_reach_git() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    let gh = fake_gh(tmp.path(), true);

    let error = github::connect_repository(Some(&gh), &repo, "octo/demo;rm -rf").await.unwrap_err();
    assert!(error.to_string().contains("is not a GitHub owner/repository name"), "{error}");
    assert_eq!(origin(&repo), None);
}

#[tokio::test]
async fn origin_credentials_are_stripped_before_reporting() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    git(&repo, &["remote", "add", "origin", "https://user:ghp_leaked@github.com/octo/demo.git"]);
    let gh = fake_gh(tmp.path(), true);

    let connection = github::repo_connection(Some(&gh), &repo).await.unwrap();
    assert_eq!(connection.state, LinkState::Linked);
    assert_eq!(connection.remote_url.as_deref(), Some("https://github.com/octo/demo.git"));
}

#[tokio::test]
async fn origin_is_unverified_when_gh_is_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = make_repo(tmp.path());
    git(&repo, &["remote", "add", "origin", "https://github.com/octo/demo.git"]);

    let connection = github::repo_connection(None, &repo).await.unwrap();
    assert_eq!(connection.state, LinkState::Unverified);
    let detail = connection.detail.expect("missing CLI is explained");
    assert!(detail.contains("not found on your PATH"), "{detail}");
}

#[tokio::test]
async fn picked_repository_is_cloned_into_the_managed_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("OpenConductor").join("repos");
    let gh = fake_gh(tmp.path(), true);

    let path = github::clone_repository(Some(&gh), &root, "octo/demo").await.unwrap();
    assert_eq!(path, root.join("demo"));
    assert!(path.join(".git").exists());
    assert_eq!(
        git(&path, &["config", "--get", "remote.origin.url"]).trim(),
        "https://github.com/octo/demo.git"
    );
}

#[tokio::test]
async fn existing_checkout_of_the_same_repository_is_reused_without_gh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repos");
    let gh = fake_gh(tmp.path(), true);
    let first = github::clone_repository(Some(&gh), &root, "octo/demo").await.unwrap();

    // With no gh available at all, reuse must still succeed: nothing is fetched.
    let again = github::clone_repository(None, &root, "octo/demo").await.unwrap();
    assert_eq!(again, first);
}

#[tokio::test]
async fn folder_holding_a_different_repository_is_never_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repos");
    let other = root.join("demo");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    git(&other, &["remote", "add", "origin", "https://github.com/someone/else.git"]);
    let gh = fake_gh(tmp.path(), true);

    let error = github::clone_repository(Some(&gh), &root, "octo/demo").await.unwrap_err();
    assert!(error.to_string().contains("already holds a different repository"), "{error}");
    assert_eq!(
        git(&other, &["config", "--get", "remote.origin.url"]).trim(),
        "https://github.com/someone/else.git"
    );
}

#[tokio::test]
async fn non_empty_plain_folder_blocks_the_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repos");
    let folder = root.join("demo");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), "keep me").unwrap();
    let gh = fake_gh(tmp.path(), true);

    let error = github::clone_repository(Some(&gh), &root, "octo/demo").await.unwrap_err();
    assert!(error.to_string().contains("is not a checkout of octo/demo"), "{error}");
    assert!(folder.join("notes.txt").exists());
}

#[tokio::test]
async fn clone_fails_cleanly_when_the_account_cannot_see_the_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repos");
    let gh = fake_gh(tmp.path(), true);

    let error = github::clone_repository(Some(&gh), &root, "someone/else").await.unwrap_err();
    assert!(error.to_string().contains("Could not resolve"), "{error}");
    assert!(!root.join("else").exists());
}

#[tokio::test]
async fn clone_requires_gh_when_the_repository_is_not_already_here() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repos");

    let error = github::clone_repository(None, &root, "octo/demo").await.unwrap_err();
    assert!(error.to_string().contains("not found on your PATH"), "{error}");
}

#[tokio::test]
async fn invalid_names_are_rejected_before_any_folder_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repos");
    let gh = fake_gh(tmp.path(), true);

    let error = github::clone_repository(Some(&gh), &root, "../../etc").await.unwrap_err();
    assert!(error.to_string().contains("is not a GitHub owner/repository name"), "{error}");
    assert!(!root.exists());
}

#[tokio::test]
async fn account_repositories_are_listed_for_the_picker() {
    let tmp = tempfile::tempdir().unwrap();
    let gh = fake_gh(tmp.path(), true);
    let repos = github::list_repositories(Some(&gh), tmp.path()).await.unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name_with_owner, "octo/demo");
    assert_eq!(repos[0].description, None);
}
