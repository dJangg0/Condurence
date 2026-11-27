//! Remotes and their branches, for the repository settings page, and the GitHub repository a
//! remote points at, so `gh` can be told which one to use.

use std::path::Path;

use crate::error::AppResult;
use crate::git::cmd;

/// The names of the repository's remotes, in git's order.
pub async fn list_remotes(repo: &Path) -> AppResult<Vec<String>> {
    let text = cmd::run(repo, &["remote"]).await?;
    Ok(text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect())
}

/// The remote-tracking branches of `remote`, as `remote/branch`. It fetches first so the list is
/// current. A failed fetch, such as being offline, still lists what is already known.
pub async fn list_remote_branches(repo: &Path, remote: &str) -> AppResult<Vec<String>> {
    let _ = cmd::raw(repo, &["fetch", "--quiet", "--no-tags", remote]).await;
    let prefix = format!("refs/remotes/{remote}/");
    let text = cmd::run(repo, &["for-each-ref", "--format=%(refname:short)", prefix.as_str()]).await?;
    let head = format!("{remote}/HEAD");
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != head)
        .map(str::to_string)
        .collect())
}

/// The `owner/name` of a GitHub remote URL, in either SSH or HTTPS form. Other hosts give `None`.
pub fn github_slug(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = url
        .split_once("github.com:")
        .or_else(|| url.split_once("github.com/"))
        .map(|(_, rest)| rest)?;
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

/// Arguments that point `gh` at the GitHub repository behind `remote`. Empty when the remote is
/// not on GitHub, which leaves `gh` to find the repository itself.
pub async fn gh_repo_args(repo: &Path, remote: &str) -> Vec<String> {
    match cmd::run(repo, &["remote", "get-url", remote]).await {
        Ok(url) => github_slug(&url).map(|slug| vec!["--repo".to_string(), slug]).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls_give_their_owner_and_repository() {
        assert_eq!(github_slug("git@github.com:dJangg0/Condurrence.git").as_deref(), Some("dJangg0/Condurrence"));
        assert_eq!(github_slug("https://github.com/acme/demo").as_deref(), Some("acme/demo"));
        assert_eq!(github_slug("https://github.com/acme/demo.git/").as_deref(), Some("acme/demo"));
    }

    #[test]
    fn other_hosts_and_malformed_urls_have_no_slug() {
        assert_eq!(github_slug("/Users/me/remote.git"), None);
        assert_eq!(github_slug("https://gitlab.com/acme/demo"), None);
        assert_eq!(github_slug("https://github.com/acme"), None);
        assert_eq!(github_slug("https://github.com/acme/demo/extra"), None);
    }
}
