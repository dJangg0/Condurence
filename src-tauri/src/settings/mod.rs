//! Per-repository Git settings, laid out like Conductor's Git page: where new workspaces branch
//! from, which remote receives pushes and PRs, and what archiving and merging do to branches and
//! workspaces. Stored per repository in the state database as JSON.

use serde::{Deserialize, Serialize};

use crate::error::AppResult;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct RepoGitSettings {
    /// Remote-tracking branch that new workspaces start from, such as `origin/main`.
    pub base_ref: String,
    /// Remote that receives pushes and PRs, and that fetches are made from.
    pub remote: String,
    /// Delete the local branch when a workspace is archived or removed.
    pub delete_branch_on_archive: bool,
    /// Archive a workspace automatically once its PR is merged.
    pub archive_on_merge: bool,
    /// New workspaces set an upstream on a plain `git push`.
    pub set_upstream_on_push: bool,
}

impl Default for RepoGitSettings {
    fn default() -> Self {
        Self {
            base_ref: "origin/main".into(),
            remote: "origin".into(),
            delete_branch_on_archive: false,
            archive_on_merge: false,
            set_upstream_on_push: true,
        }
    }
}

impl RepoGitSettings {
    /// The base branch without its remote, so `origin/release/1.0` becomes `release/1.0`. Remote
    /// names cannot contain a slash, so the first slash separates them.
    pub fn base_branch(&self) -> String {
        self.base_ref
            .split_once('/')
            .map(|(_, branch)| branch)
            .unwrap_or(&self.base_ref)
            .to_string()
    }

    /// Checks the values that reach git as arguments before they are saved.
    pub fn validate(&self) -> AppResult<()> {
        if !valid_name(&self.remote) {
            crate::error::bail!("'{}' is not a valid remote name", self.remote);
        }
        let Some((remote, branch)) = self.base_ref.split_once('/') else {
            crate::error::bail!("the base must look like origin/main, not '{}'", self.base_ref);
        };
        if remote != self.remote || !valid_name(branch) {
            crate::error::bail!("the base '{}' must be a branch on the '{}' remote", self.base_ref, self.remote);
        }
        Ok(())
    }
}

/// A name that is safe to pass to git: no leading dash, no spaces or control characters, no `..`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

/// Settings from the stored JSON. Missing or unreadable values fall back to the defaults, so a
/// damaged record never blocks a repository.
pub fn parse(stored: Option<&str>) -> RepoGitSettings {
    stored
        .and_then(|json| serde_json::from_str::<RepoGitSettings>(json).ok())
        .filter(|settings| settings.validate().is_ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_conductor_git_page() {
        let defaults = RepoGitSettings::default();
        assert_eq!(defaults.base_ref, "origin/main");
        assert_eq!(defaults.remote, "origin");
        assert!(!defaults.delete_branch_on_archive);
        assert!(!defaults.archive_on_merge);
        assert!(defaults.set_upstream_on_push);
    }

    #[test]
    fn the_base_branch_drops_only_the_remote_prefix() {
        let mut settings = RepoGitSettings::default();
        assert_eq!(settings.base_branch(), "main");
        settings.base_ref = "origin/release/1.0".into();
        assert_eq!(settings.base_branch(), "release/1.0");
    }

    #[test]
    fn values_that_could_read_as_options_are_refused() {
        let settings = RepoGitSettings { remote: "--upload-pack=x".into(), ..RepoGitSettings::default() };
        assert!(settings.validate().is_err());

        let settings = RepoGitSettings { base_ref: "upstream/main".into(), ..RepoGitSettings::default() };
        assert!(settings.validate().is_err(), "the base must be on the chosen remote");

        let settings = RepoGitSettings { base_ref: "origin/bad branch".into(), ..RepoGitSettings::default() };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn stored_json_round_trips_and_missing_fields_take_defaults() {
        let settings = RepoGitSettings { archive_on_merge: true, ..RepoGitSettings::default() };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(parse(Some(&json)), settings);

        assert_eq!(parse(Some(r#"{"archiveOnMerge":true}"#)).remote, "origin");
        assert!(parse(Some(r#"{"archiveOnMerge":true}"#)).archive_on_merge);
    }

    #[test]
    fn damaged_or_invalid_records_fall_back_to_the_defaults() {
        assert_eq!(parse(None), RepoGitSettings::default());
        assert_eq!(parse(Some("not json")), RepoGitSettings::default());
        assert_eq!(parse(Some(r#"{"remote":"--evil"}"#)), RepoGitSettings::default());
    }
}
