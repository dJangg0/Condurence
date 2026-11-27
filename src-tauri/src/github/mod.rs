//! GitHub connection, built on the `gh` CLI the user already signs in with.
//!
//! Condurence never reads or stores a GitHub token. Sign-in state comes from
//! `gh auth status`, the account from `gh api user`, and access to a repository from
//! `gh repo view`. Linking a repository only adds an `origin` remote with git, and only
//! after `gh` has confirmed the signed-in account can see that repository.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::git;

/// The only host this module talks to.
pub const HOST: &str = "github.com";

const GH_TIMEOUT: Duration = Duration::from_secs(20);
/// Cloning large repositories can take minutes, so it gets a longer budget than metadata calls.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);
pub const GH_MISSING: &str =
    "The GitHub CLI (gh) was not found on your PATH. Install it with `brew install gh`, then refresh.";

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    /// False when the `gh` binary cannot be found.
    pub cli_installed: bool,
    pub signed_in: bool,
    pub login: Option<String>,
    pub name: Option<String>,
}

impl GitHubAccount {
    fn not_installed() -> Self {
        Self { cli_installed: false, signed_in: false, login: None, name: None }
    }

    fn signed_out() -> Self {
        Self { cli_installed: true, signed_in: false, login: None, name: None }
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LinkState {
    /// `origin` is a GitHub remote and `gh` confirmed the signed-in account can see it.
    Linked,
    /// `origin` is a GitHub remote that `gh` could not confirm. `detail` says why.
    Unverified,
    /// The repository has no `origin` remote yet.
    Unlinked,
    /// `origin` points at a host other than github.com.
    OtherRemote,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepoConnection {
    pub state: LinkState,
    /// The `origin` URL with any embedded credentials removed.
    pub remote_url: Option<String>,
    pub name_with_owner: Option<String>,
    pub web_url: Option<String>,
    pub is_private: Option<bool>,
    pub viewer_permission: Option<String>,
    pub default_branch: Option<String>,
    pub detail: Option<String>,
}

impl RepoConnection {
    fn without_github(state: LinkState, remote_url: Option<String>, detail: Option<String>) -> Self {
        Self {
            state,
            remote_url,
            name_with_owner: None,
            web_url: None,
            is_private: None,
            viewer_permission: None,
            default_branch: None,
            detail,
        }
    }
}

/// One repository from the signed-in account, for the connect picker.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepoSummary {
    pub name_with_owner: String,
    pub url: String,
    pub is_private: bool,
    pub description: Option<String>,
}

/// The subset of `gh repo view --json` that Condurence reads.
#[derive(Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct GhRepoView {
    name_with_owner: String,
    url: String,
    is_private: bool,
    viewer_permission: Option<String>,
    default_branch_ref: Option<GhBranchRef>,
}

#[derive(Deserialize, Debug, PartialEq, Eq)]
struct GhBranchRef {
    name: String,
}

/// Arguments for `gh` that start the interactive GitHub.com sign-in. Run inside a PTY.
pub fn sign_in_args() -> Vec<String> {
    ["auth", "login", "--hostname", HOST, "--web"].into_iter().map(String::from).collect()
}

/// Signed-in state and the account behind it. `gh` is `None` when the CLI is not installed.
pub async fn account_status(gh: Option<&Path>, cwd: &Path) -> AppResult<GitHubAccount> {
    let Some(gh) = gh else {
        return Ok(GitHubAccount::not_installed());
    };
    let status = run_gh(gh, cwd, &["auth", "status", "--hostname", HOST]).await?;
    if !status.status.success() {
        return Ok(GitHubAccount::signed_out());
    }
    let user = run_gh(gh, cwd, &["api", "--hostname", HOST, "user"]).await?;
    if !user.status.success() {
        return Ok(GitHubAccount::signed_out());
    }
    Ok(parse_user(&String::from_utf8_lossy(&user.stdout)).unwrap_or_else(GitHubAccount::signed_out))
}

/// How the repository's `origin` relates to GitHub, verified against `gh` when possible.
pub async fn repo_connection(gh: Option<&Path>, repo_root: &Path) -> AppResult<RepoConnection> {
    let Some(remote) = origin_url(repo_root).await? else {
        return Ok(RepoConnection::without_github(LinkState::Unlinked, None, None));
    };
    let shown = Some(redact(&remote));
    let Some(slug) = parse_github_slug(&remote) else {
        let detail = "origin points at a host other than github.com, so it is not linked to GitHub.".to_string();
        return Ok(RepoConnection::without_github(LinkState::OtherRemote, shown, Some(detail)));
    };
    let Some(gh) = gh else {
        return Ok(RepoConnection::without_github(
            LinkState::Unverified,
            shown,
            Some(GH_MISSING.to_string()),
        ));
    };
    match fetch_repo(gh, repo_root, &slug).await {
        Ok(view) => Ok(connection_from_view(view, shown)),
        Err(detail) => Ok(RepoConnection::without_github(LinkState::Unverified, shown, Some(detail))),
    }
}

/// Repositories the signed-in account owns, most recently pushed first.
pub async fn list_repositories(gh: Option<&Path>, cwd: &Path) -> AppResult<Vec<RepoSummary>> {
    let Some(gh) = gh else {
        crate::error::bail!("{GH_MISSING}");
    };
    let output = run_gh(
        gh,
        cwd,
        &["repo", "list", "--limit", "200", "--json", "nameWithOwner,url,isPrivate,description"],
    )
    .await?;
    if !output.status.success() {
        crate::error::bail!("{}", first_line(&output.stderr, "gh repo list failed"));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| AppError::msg(format!("unexpected gh output: {e}")))
}

/// Adds `origin` pointing at a GitHub repository the signed-in account can access.
///
/// Refuses to replace an existing `origin`. Calling it again for the repository that is
/// already linked simply reports its current state.
pub async fn connect_repository(gh: Option<&Path>, repo_root: &Path, name_with_owner: &str) -> AppResult<RepoConnection> {
    if !is_valid_slug(name_with_owner) {
        crate::error::bail!("'{name_with_owner}' is not a GitHub owner/repository name");
    }
    if let Some(existing) = origin_url(repo_root).await? {
        if parse_github_slug(&existing).is_some_and(|slug| slug.eq_ignore_ascii_case(name_with_owner)) {
            return repo_connection(gh, repo_root).await;
        }
        crate::error::bail!(
            "origin already points to {}. Remove or rename that remote before linking another repository.",
            redact(&existing)
        );
    }
    let Some(gh) = gh else {
        crate::error::bail!("{GH_MISSING}");
    };
    let view = fetch_repo(gh, repo_root, name_with_owner).await.map_err(AppError::msg)?;
    let protocol = git_protocol(gh, repo_root).await;
    let url = clone_url(&view.name_with_owner, &protocol);
    git::cmd::run(repo_root, &["remote", "add", "origin", &url]).await?;
    repo_connection(Some(gh), repo_root).await
}

/// Folder that repositories picked from GitHub are cloned into: `~/Condurence/repos`.
pub fn default_clone_root(home: &Path) -> PathBuf {
    home.join("Condurence").join("repos")
}

/// Folder repositories were cloned into under the app's earlier name: `~/OpenConductor/repos`.
/// Checkouts made there stay where they are, because the app remembers them by path, and picking
/// the same repository again finds them with `existing_clone` instead of cloning it a second time.
pub fn legacy_clone_root(home: &Path) -> PathBuf {
    home.join("OpenConductor").join("repos")
}

/// The checkout of `name_with_owner` that `root` already holds, if it holds one. A folder of that
/// name that is another repository, or no repository, is not a match.
pub async fn existing_clone(root: &Path, name_with_owner: &str) -> Option<PathBuf> {
    if !is_valid_slug(name_with_owner) {
        return None;
    }
    let target = root.join(name_with_owner.split('/').nth(1)?);
    if !target.join(".git").exists() {
        return None;
    }
    let origin = origin_url(&target).await.ok().flatten()?;
    parse_github_slug(&origin)
        .is_some_and(|slug| slug.eq_ignore_ascii_case(name_with_owner))
        .then_some(target)
}

/// Clones `name_with_owner` into `root/<name>` with `gh repo clone` and returns that path.
///
/// If the folder already holds a checkout of the same repository, that checkout is returned
/// without touching the network, so opening a repository twice never clones it twice.
pub async fn clone_repository(gh: Option<&Path>, root: &Path, name_with_owner: &str) -> AppResult<PathBuf> {
    if !is_valid_slug(name_with_owner) {
        crate::error::bail!("'{name_with_owner}' is not a GitHub owner/repository name");
    }
    let name = name_with_owner.split('/').nth(1).unwrap_or_default();
    let target = root.join(name);

    if target.join(".git").exists() {
        let origin = origin_url(&target).await?;
        let same = origin
            .as_deref()
            .and_then(parse_github_slug)
            .is_some_and(|slug| slug.eq_ignore_ascii_case(name_with_owner));
        if same {
            return Ok(target);
        }
        crate::error::bail!("{} already holds a different repository", target.display());
    }
    if target.exists() && !is_empty_dir(&target) {
        crate::error::bail!("{} already exists and is not a checkout of {name_with_owner}", target.display());
    }

    let Some(gh) = gh else {
        crate::error::bail!("{GH_MISSING}");
    };
    std::fs::create_dir_all(root)?;
    let target_arg = target.to_string_lossy().into_owned();
    let output = run_gh_within(gh, root, &["repo", "clone", name_with_owner, target_arg.as_str()], CLONE_TIMEOUT).await?;
    if !output.status.success() {
        crate::error::bail!("{}", first_line(&output.stderr, "gh repo clone failed"));
    }
    Ok(target)
}

fn is_empty_dir(path: &Path) -> bool {
    path.is_dir() && std::fs::read_dir(path).map(|mut entries| entries.next().is_none()).unwrap_or(false)
}

fn connection_from_view(view: GhRepoView, remote_url: Option<String>) -> RepoConnection {
    RepoConnection {
        state: LinkState::Linked,
        remote_url,
        name_with_owner: Some(view.name_with_owner),
        web_url: Some(view.url),
        is_private: Some(view.is_private),
        viewer_permission: view.viewer_permission,
        default_branch: view.default_branch_ref.map(|branch| branch.name),
        detail: None,
    }
}

/// Runs `gh repo view`. On failure returns the reason as user-facing text.
async fn fetch_repo(gh: &Path, cwd: &Path, slug: &str) -> Result<GhRepoView, String> {
    let output = run_gh(
        gh,
        cwd,
        &[
            "repo",
            "view",
            slug,
            "--json",
            "nameWithOwner,url,isPrivate,viewerPermission,defaultBranchRef",
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(first_line(&output.stderr, "gh could not access this repository"));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("unexpected gh output: {e}"))
}

async fn git_protocol(gh: &Path, cwd: &Path) -> String {
    match run_gh(gh, cwd, &["config", "get", "git_protocol", "--host", HOST]).await {
        Ok(output) if output.status.success() => {
            if String::from_utf8_lossy(&output.stdout).trim() == "ssh" {
                "ssh".to_string()
            } else {
                "https".to_string()
            }
        }
        _ => "https".to_string(),
    }
}

/// Runs `gh` non-interactively with a timeout. Prompts are disabled, so a missing login
/// fails fast instead of waiting on stdin.
async fn run_gh(gh: &Path, cwd: &Path, args: &[&str]) -> AppResult<Output> {
    run_gh_within(gh, cwd, args, GH_TIMEOUT).await
}

/// Shared with `pulls`, which runs the same `gh` calls for pull requests.
pub(crate) async fn run_gh_within(gh: &Path, cwd: &Path, args: &[&str], limit: Duration) -> AppResult<Output> {
    let mut command = Command::new(gh);
    command
        .current_dir(cwd)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(limit, command.output()).await {
        Ok(result) => result.map_err(|e| AppError::msg(format!("failed to run gh: {e}"))),
        Err(_) => crate::error::bail!("gh {} timed out", args.first().copied().unwrap_or("")),
    }
}

async fn origin_url(repo_root: &Path) -> AppResult<Option<String>> {
    let output = git::cmd::raw(repo_root, &["config", "--get", "remote.origin.url"]).await?;
    if !output.status.success() {
        return Ok(None);
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!url.is_empty()).then_some(url))
}

fn parse_user(json: &str) -> Option<GitHubAccount> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let login = value.get("login")?.as_str()?.trim().to_string();
    if login.is_empty() {
        return None;
    }
    let name = value
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    Some(GitHubAccount { cli_installed: true, signed_in: true, login: Some(login), name })
}

/// Extracts `owner/name` from a github.com remote. Accepts HTTPS, SSH URLs and scp-style
/// `git@github.com:owner/name.git` forms. Any other host returns `None`.
pub fn parse_github_slug(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let (authority, path) = match remote.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => remote.split_once(':')?,
    };
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if !host.eq_ignore_ascii_case(HOST) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let slug = format!("{owner}/{name}");
    is_valid_slug(&slug).then_some(slug)
}

/// True for `owner/name` made of the characters GitHub allows in account and repo names.
pub fn is_valid_slug(slug: &str) -> bool {
    let Some((owner, name)) = slug.split_once('/') else {
        return false;
    };
    // An owner never starts with '-', and refusing it keeps the slug from reading as an option
    // when it is passed to `gh` as a positional argument.
    let owner_ok = !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    owner_ok && name_ok
}

/// Removes `user:secret@` from a URL so credentials never reach the UI.
pub fn redact(remote: &str) -> String {
    let Some((scheme, rest)) = remote.split_once("://") else {
        return remote.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}{tail}"),
        None => remote.to_string(),
    }
}

fn clone_url(slug: &str, protocol: &str) -> String {
    if protocol == "ssh" {
        format!("git@{HOST}:{slug}.git")
    } else {
        format!("https://{HOST}/{slug}.git")
    }
}

fn first_line(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    if line.is_empty() {
        fallback.to_string()
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_remotes_in_every_common_form() {
        let cases = [
            "https://github.com/dJangg0/Condurrence.git",
            "https://github.com/dJangg0/Condurrence",
            "https://github.com/dJangg0/Condurrence/",
            "http://GitHub.com/dJangg0/Condurrence.git",
            "https://user:ghp_secret@github.com/dJangg0/Condurrence.git",
            "git@github.com:dJangg0/Condurrence.git",
            "ssh://git@github.com/dJangg0/Condurrence.git",
            "ssh://git@github.com:22/dJangg0/Condurrence.git",
        ];
        for remote in cases {
            assert_eq!(parse_github_slug(remote).as_deref(), Some("dJangg0/Condurrence"), "{remote}");
        }
    }

    #[test]
    fn rejects_other_hosts_and_non_repository_paths() {
        let cases = [
            "https://gitlab.com/owner/repo.git",
            "git@bitbucket.org:owner/repo.git",
            "https://github.com.evil.example/owner/repo.git",
            "https://github.com/owner",
            "https://github.com/owner/repo/extra",
            "/Users/me/src/repo",
            "file:///tmp/repo",
            "C:\\repos\\demo",
        ];
        for remote in cases {
            assert_eq!(parse_github_slug(remote), None, "{remote}");
        }
    }

    #[test]
    fn validates_owner_and_repository_names() {
        assert!(is_valid_slug("dJangg0/Condurrence"));
        assert!(is_valid_slug("some-org/my.repo_v2"));
        assert!(!is_valid_slug("owner"));
        assert!(!is_valid_slug("owner/"));
        assert!(!is_valid_slug("/repo"));
        assert!(!is_valid_slug("owner/../etc"));
        assert!(!is_valid_slug("owner/a/b"));
        assert!(!is_valid_slug("own er/repo"));
        assert!(!is_valid_slug("-flag/repo.git;rm"));
        assert!(!is_valid_slug("--limit/repo"), "an owner must not read as a gh option");
        assert!(!is_valid_slug("-x/repo"));
    }

    #[test]
    fn redacts_credentials_but_keeps_the_rest_of_the_url() {
        assert_eq!(
            redact("https://user:ghp_secret@github.com/dJangg0/Condurrence.git"),
            "https://github.com/dJangg0/Condurrence.git"
        );
        assert_eq!(redact("https://github.com/a/b.git"), "https://github.com/a/b.git");
        assert_eq!(redact("git@github.com:a/b.git"), "git@github.com:a/b.git");
    }

    #[test]
    fn builds_clone_urls_for_the_configured_protocol() {
        assert_eq!(clone_url("dJangg0/Condurrence", "https"), "https://github.com/dJangg0/Condurrence.git");
        assert_eq!(clone_url("dJangg0/Condurrence", "ssh"), "git@github.com:dJangg0/Condurrence.git");
    }

    #[test]
    fn parses_gh_user_output() {
        let json = r#"{"login":"dJangg0","id":125983424,"name":"Leandro Jose "}"#;
        let account = parse_user(json).expect("user should parse");
        assert!(account.signed_in);
        assert_eq!(account.login.as_deref(), Some("dJangg0"));
        assert_eq!(account.name.as_deref(), Some("Leandro Jose"));

        let unnamed = parse_user(r#"{"login":"octocat","name":null}"#).unwrap();
        assert_eq!(unnamed.name, None);
        assert_eq!(parse_user("not json"), None);
    }

    #[test]
    fn parses_gh_repo_view_output_including_empty_default_branch() {
        let json = r#"{"defaultBranchRef":null,"isPrivate":true,"nameWithOwner":"o/r","url":"https://github.com/o/r","viewerPermission":"WRITE"}"#;
        let view: GhRepoView = serde_json::from_str(json).expect("view should parse");
        let connection = connection_from_view(view, None);
        assert_eq!(connection.state, LinkState::Linked);
        assert_eq!(connection.is_private, Some(true));
        assert_eq!(connection.viewer_permission.as_deref(), Some("WRITE"));
        assert_eq!(connection.default_branch, None);
    }

    #[test]
    fn picked_repositories_clone_under_the_home_folder() {
        assert_eq!(
            default_clone_root(Path::new("/Users/me")),
            PathBuf::from("/Users/me/Condurence/repos")
        );
    }

    #[test]
    fn sign_in_uses_the_web_flow_for_github_com() {
        assert_eq!(sign_in_args(), vec!["auth", "login", "--hostname", "github.com", "--web"]);
    }
}
