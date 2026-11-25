//! Pull requests for a workspace branch, through the `gh` CLI.
//!
//! Condurence never reads a GitHub token. State comes from `gh pr view`. Creating and merging
//! a PR use `gh pr create` and `gh pr merge`, and `git` pushes the branch and brings the base
//! branch into it.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::git;
use crate::github;

const PR_TIMEOUT: Duration = Duration::from_secs(60);
const PULL_FIELDS: &str = "number,url,state,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup";

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChecksState {
    /// The PR has no checks configured.
    None,
    Pending,
    Passing,
    Failing,
}

/// The fields of a PR that drive the workspace's primary button. States stay as GitHub spells
/// them, so the frontend can match them directly.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStatus {
    pub number: u64,
    pub url: String,
    /// `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    pub is_draft: bool,
    /// `MERGEABLE`, `CONFLICTING`, or `UNKNOWN` while GitHub computes it.
    pub mergeable: String,
    pub merge_state_status: String,
    pub review_decision: Option<String>,
    pub checks: ChecksState,
}

/// What `sync_with_base` did. `conflicts` lists the files still unmerged when it stopped.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub pushed: bool,
    pub conflicts: Vec<String>,
    pub message: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GhPull {
    number: u64,
    url: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    mergeable: String,
    #[serde(default)]
    merge_state_status: String,
    review_decision: Option<String>,
    #[serde(default)]
    status_check_rollup: Vec<GhCheck>,
}

/// One `statusCheckRollup` entry. Check runs carry `status` and `conclusion`. Commit statuses
/// carry `state`.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GhCheck {
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub state: Option<String>,
}

enum Outcome {
    Passing,
    Pending,
    Failing,
}

fn check_outcome(check: &GhCheck) -> Outcome {
    if let Some(status) = check.status.as_deref() {
        if status != "COMPLETED" {
            return Outcome::Pending;
        }
        return match check.conclusion.as_deref() {
            Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "STARTUP_FAILURE" | "ACTION_REQUIRED") => Outcome::Failing,
            // SUCCESS, NEUTRAL, SKIPPED and STALE do not block a merge.
            _ => Outcome::Passing,
        };
    }
    match check.state.as_deref() {
        Some("PENDING" | "EXPECTED") => Outcome::Pending,
        Some("FAILURE" | "ERROR") => Outcome::Failing,
        _ => Outcome::Passing,
    }
}

/// Collapses the check list into one state. Any failure wins, then anything still running.
pub fn classify_checks(checks: &[GhCheck]) -> ChecksState {
    if checks.is_empty() {
        return ChecksState::None;
    }
    let mut pending = false;
    for check in checks {
        match check_outcome(check) {
            Outcome::Failing => return ChecksState::Failing,
            Outcome::Pending => pending = true,
            Outcome::Passing => {}
        }
    }
    if pending {
        ChecksState::Pending
    } else {
        ChecksState::Passing
    }
}

fn to_status(pull: GhPull) -> PullRequestStatus {
    PullRequestStatus {
        number: pull.number,
        url: pull.url,
        state: pull.state,
        is_draft: pull.is_draft,
        mergeable: if pull.mergeable.is_empty() { "UNKNOWN".into() } else { pull.mergeable },
        merge_state_status: if pull.merge_state_status.is_empty() {
            "UNKNOWN".into()
        } else {
            pull.merge_state_status
        },
        review_decision: pull.review_decision.filter(|decision| !decision.is_empty()),
        checks: classify_checks(&pull.status_check_rollup),
    }
}

/// Parses one `gh pr view --json` object.
pub fn parse_pull(json: &[u8]) -> AppResult<PullRequestStatus> {
    let pull: GhPull = serde_json::from_slice(json)?;
    Ok(to_status(pull))
}

/// The PR for `branch`, or `None` when no PR was ever opened from it.
pub async fn status(gh: Option<&Path>, worktree: &Path, branch: &str) -> AppResult<Option<PullRequestStatus>> {
    let gh = gh.ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    validate_branch(branch)?;
    let output = github::run_gh_within(gh, worktree, &["pr", "view", branch, "--json", PULL_FIELDS], PR_TIMEOUT).await?;
    if !output.status.success() {
        if String::from_utf8_lossy(&output.stderr).contains("no pull requests found") {
            return Ok(None);
        }
        crate::error::bail!("{}", first_line(&output.stderr, "gh pr view failed"));
    }
    parse_pull(&output.stdout).map(Some)
}

/// Pushes `branch` and opens a PR into `base` from its commits.
pub async fn create(gh: Option<&Path>, worktree: &Path, branch: &str, base: &str) -> AppResult<PullRequestStatus> {
    let gh = gh.ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    validate_branch(branch)?;
    validate_branch(base)?;
    // Push first, so `gh pr create` never has to ask about an unpushed branch.
    git::cmd::run(worktree, &["push", "--set-upstream", "origin", branch]).await?;
    let output = github::run_gh_within(
        gh,
        worktree,
        &["pr", "create", "--base", base, "--head", branch, "--fill"],
        PR_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        crate::error::bail!("{}", first_line(&output.stderr, "gh pr create failed"));
    }
    status(Some(gh), worktree, branch)
        .await?
        .ok_or_else(|| AppError::msg("the PR was created, but gh could not read it back"))
}

/// Squash-merges the PR for `branch`. The check runs here as well as on GitHub, so the app
/// never asks for a merge it already knows will be refused.
pub async fn merge(gh: Option<&Path>, worktree: &Path, branch: &str) -> AppResult<PullRequestStatus> {
    let gh = gh.ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    let current = status(Some(gh), worktree, branch)
        .await?
        .ok_or_else(|| AppError::msg("This branch has no pull request yet."))?;
    if let Some(reason) = merge_blocker(&current) {
        crate::error::bail!("{reason}");
    }
    let number = current.number.to_string();
    let output =
        github::run_gh_within(gh, worktree, &["pr", "merge", number.as_str(), "--squash"], PR_TIMEOUT).await?;
    if !output.status.success() {
        crate::error::bail!("{}", first_line(&output.stderr, "gh pr merge failed"));
    }
    status(Some(gh), worktree, branch)
        .await?
        .ok_or_else(|| AppError::msg("the PR was merged, but gh could not read it back"))
}

/// Why a PR cannot be merged yet, or `None` when it can.
fn merge_blocker(pr: &PullRequestStatus) -> Option<String> {
    match pr.state.as_str() {
        "OPEN" => {}
        "MERGED" => return Some("This PR is already merged.".into()),
        _ => return Some("This PR is closed.".into()),
    }
    if pr.is_draft {
        return Some("Mark the PR ready for review before merging.".into());
    }
    if pr.mergeable == "CONFLICTING" || pr.merge_state_status == "DIRTY" {
        return Some("The PR has conflicts. Resolve them first.".into());
    }
    if pr.mergeable != "MERGEABLE" {
        return Some("GitHub is still checking mergeability. Try again in a moment.".into());
    }
    if pr.merge_state_status == "BLOCKED" {
        return Some("Branch protection is blocking this merge. Approve it or meet the requirement on GitHub.".into());
    }
    match pr.checks {
        ChecksState::Failing => Some("Checks are failing.".into()),
        ChecksState::Pending => Some("Checks are still running.".into()),
        ChecksState::None | ChecksState::Passing => None,
    }
}

/// Brings `base` into the branch and pushes it, so the PR picks up the merge.
///
/// If `base` conflicts with the branch, the merge stays open and `conflicts` lists the files.
/// The caller then starts an agent to resolve them. The next call finishes the merge once the
/// conflicts are gone.
pub async fn sync_with_base(worktree: &Path, branch: &str, base: &str) -> AppResult<SyncOutcome> {
    validate_branch(branch)?;
    validate_branch(base)?;
    let merging = git::cmd::raw(worktree, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .await?
        .status
        .success();

    if merging {
        let conflicts = unmerged_files(worktree).await?;
        if !conflicts.is_empty() {
            return Ok(SyncOutcome {
                pushed: false,
                conflicts,
                message: format!("Conflicts with origin/{base} are still open. Resolve them, then press Resolve again."),
            });
        }
        git::cmd::run(worktree, &["commit", "--no-edit"]).await?;
    } else {
        // Refuse to merge over uncommitted work. Git would carry it along into the merge commit.
        if !git::cmd::run(worktree, &["status", "--porcelain"]).await?.trim().is_empty() {
            crate::error::bail!("Commit or stash your changes before syncing with {base}.");
        }
        git::cmd::run(worktree, &["fetch", "origin", base]).await?;
        let remote_base = format!("origin/{base}");
        let merge = git::cmd::raw(worktree, &["merge", "--no-edit", remote_base.as_str()]).await?;
        if !merge.status.success() {
            let conflicts = unmerged_files(worktree).await?;
            if conflicts.is_empty() {
                crate::error::bail!("{}", first_line(&merge.stderr, "git merge failed"));
            }
            return Ok(SyncOutcome {
                pushed: false,
                conflicts,
                message: format!("origin/{base} conflicts with this branch."),
            });
        }
    }

    git::cmd::run(worktree, &["push", "origin", branch]).await?;
    Ok(SyncOutcome {
        pushed: true,
        conflicts: vec![],
        message: format!("Merged origin/{base} and pushed {branch}. Checks will run again."),
    })
}

async fn unmerged_files(worktree: &Path) -> AppResult<Vec<String>> {
    let text = git::cmd::run(worktree, &["diff", "--name-only", "--diff-filter=U"]).await?;
    Ok(text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect())
}

/// The first message an agent receives when it opens in a workspace. Only the base branch is
/// interpolated, and it is validated first.
pub fn task_prompt(task: &str, base: &str) -> AppResult<String> {
    validate_branch(base)?;
    match task {
        "review" => Ok(format!(
            "Review the changes on this branch against origin/{base}. Run `git diff origin/{base}...HEAD` \
             and read the changed files. List correctness bugs, risky changes and missing tests, most severe \
             first, with file and line references. Do not edit files."
        )),
        "resolve" => Ok(format!(
            "This branch conflicts with origin/{base}. Run `git status` to list the conflicted files. Resolve \
             each conflict, keeping the intent of both sides, then `git add` the files and run \
             `git commit --no-edit`. Do not push. Stop and ask if a conflict is unclear."
        )),
        other => crate::error::bail!("'{other}' is not an agent task"),
    }
}

/// Branch names reach git and gh as arguments. Refuse anything that could read as an option or
/// that git would reject outright.
fn validate_branch(name: &str) -> AppResult<()> {
    let forbidden = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\');
    let ok = !name.is_empty() && !name.starts_with('-') && !name.contains("..") && !name.chars().any(forbidden);
    if ok {
        Ok(())
    } else {
        crate::error::bail!("'{name}' is not a valid branch name")
    }
}

fn first_line(stderr: &[u8], fallback: &str) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGED_PR: &str = r#"{"isDraft":false,"mergeStateStatus":"UNKNOWN","mergeable":"UNKNOWN","number":1,"reviewDecision":"","state":"MERGED","statusCheckRollup":[],"url":"https://github.com/o/r/pull/1"}"#;

    fn run(status: Option<&str>, conclusion: Option<&str>) -> GhCheck {
        GhCheck { status: status.map(String::from), conclusion: conclusion.map(String::from), state: None }
    }

    fn commit_status(state: &str) -> GhCheck {
        GhCheck { status: None, conclusion: None, state: Some(state.into()) }
    }

    fn open_pr() -> PullRequestStatus {
        PullRequestStatus {
            number: 7,
            url: "https://github.com/o/r/pull/7".into(),
            state: "OPEN".into(),
            is_draft: false,
            mergeable: "MERGEABLE".into(),
            merge_state_status: "CLEAN".into(),
            review_decision: None,
            checks: ChecksState::Passing,
        }
    }

    #[test]
    fn parses_a_merged_pr_with_no_checks_and_empty_review_decision() {
        let pr = parse_pull(MERGED_PR.as_bytes()).expect("gh output should parse");
        assert_eq!(pr.state, "MERGED");
        assert_eq!(pr.checks, ChecksState::None);
        assert_eq!(pr.review_decision, None);
        assert_eq!(pr.mergeable, "UNKNOWN");
    }

    #[test]
    fn no_checks_means_none_and_all_green_means_passing() {
        assert_eq!(classify_checks(&[]), ChecksState::None);
        assert_eq!(
            classify_checks(&[run(Some("COMPLETED"), Some("SUCCESS")), run(Some("COMPLETED"), Some("SKIPPED"))]),
            ChecksState::Passing
        );
        assert_eq!(classify_checks(&[commit_status("SUCCESS")]), ChecksState::Passing);
    }

    #[test]
    fn running_checks_are_pending_and_a_failure_outranks_them() {
        assert_eq!(classify_checks(&[run(Some("IN_PROGRESS"), None)]), ChecksState::Pending);
        assert_eq!(classify_checks(&[commit_status("PENDING")]), ChecksState::Pending);
        assert_eq!(
            classify_checks(&[run(Some("IN_PROGRESS"), None), run(Some("COMPLETED"), Some("FAILURE"))]),
            ChecksState::Failing
        );
        assert_eq!(classify_checks(&[commit_status("ERROR")]), ChecksState::Failing);
    }

    #[test]
    fn merge_is_refused_for_conflicts_drafts_running_or_failing_checks() {
        let mut pr = open_pr();
        assert_eq!(merge_blocker(&pr), None);

        pr.mergeable = "CONFLICTING".into();
        pr.merge_state_status = "DIRTY".into();
        assert!(merge_blocker(&pr).unwrap().contains("conflicts"));

        let mut draft = open_pr();
        draft.is_draft = true;
        assert!(merge_blocker(&draft).unwrap().contains("ready for review"));

        let mut running = open_pr();
        running.checks = ChecksState::Pending;
        assert!(merge_blocker(&running).unwrap().contains("still running"));

        let mut failing = open_pr();
        failing.checks = ChecksState::Failing;
        assert!(merge_blocker(&failing).unwrap().contains("failing"));

        let mut merged = open_pr();
        merged.state = "MERGED".into();
        assert!(merge_blocker(&merged).unwrap().contains("already merged"));
    }

    #[test]
    fn agent_tasks_are_known_and_the_base_is_checked() {
        assert!(task_prompt("review", "main").unwrap().contains("origin/main...HEAD"));
        assert!(task_prompt("resolve", "main").unwrap().contains("conflicts with origin/main"));
        assert!(task_prompt("deploy", "main").is_err());
        assert!(task_prompt("review", "main; rm -rf").is_err());
    }

    #[test]
    fn branch_names_that_could_read_as_options_or_break_git_are_refused() {
        assert!(validate_branch("dJangg0/ade-orchestration").is_ok());
        assert!(validate_branch("feature_v2.1").is_ok());
        assert!(validate_branch("--force").is_err());
        assert!(validate_branch("a..b").is_err());
        assert!(validate_branch("has space").is_err());
        assert!(validate_branch("").is_err());
    }
}
