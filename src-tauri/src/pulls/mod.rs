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
use crate::git::remotes;
use crate::github;

const PR_TIMEOUT: Duration = Duration::from_secs(60);
const PULL_FIELDS: &str = "number,url,state,isDraft,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup,headRefOid";
/// PRs read for one repository's list. Newer ones come first, so a repository past this many PRs
/// shows its older branches without a merge state.
const LIST_LIMIT: &str = "200";

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
    /// The commit at the tip of the PR's branch, as GitHub last saw it. For a merged PR this is
    /// the last commit that was merged, so it says whether the branch has moved on since.
    pub head_sha: Option<String>,
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
    #[serde(default)]
    head_ref_oid: Option<String>,
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
        head_sha: pull.head_ref_oid.filter(|sha| !sha.is_empty()),
    }
}

/// Parses one `gh pr view --json` object.
pub fn parse_pull(json: &[u8]) -> AppResult<PullRequestStatus> {
    let pull: GhPull = serde_json::from_slice(json)?;
    Ok(to_status(pull))
}

/// The PR for `branch`, or `None` when no PR was ever opened from it.
pub async fn status(
    gh: Option<&Path>,
    worktree: &Path,
    branch: &str,
    remote: &str,
) -> AppResult<Option<PullRequestStatus>> {
    let gh = gh.ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    validate_branch(branch)?;
    validate_branch(remote)?;
    let repo = remotes::gh_repo_args(worktree, remote).await;
    let mut args: Vec<&str> = vec!["pr", "view", branch, "--json", PULL_FIELDS];
    args.extend(repo.iter().map(String::as_str));
    let output = github::run_gh_within(gh, worktree, &args, PR_TIMEOUT).await?;
    if !output.status.success() {
        if String::from_utf8_lossy(&output.stderr).contains("no pull requests found") {
            return Ok(None);
        }
        crate::error::bail!("{}", first_line(&output.stderr, "gh pr view failed"));
    }
    parse_pull(&output.stdout).map(Some)
}

/// One PR from the repository's list, with the branch it was opened from.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchPull {
    pub branch: String,
    pub number: u64,
    pub url: String,
    /// `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    /// The commit at the tip of the PR's branch as GitHub last saw it.
    pub head_sha: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GhListedPull {
    number: u64,
    url: String,
    state: String,
    head_ref_name: String,
    #[serde(default)]
    head_ref_oid: Option<String>,
}

/// Parses the array `gh pr list --json` prints.
pub fn parse_list(json: &[u8]) -> AppResult<Vec<BranchPull>> {
    let pulls: Vec<GhListedPull> = serde_json::from_slice(json)?;
    Ok(pulls
        .into_iter()
        .map(|pull| BranchPull {
            branch: pull.head_ref_name,
            number: pull.number,
            url: pull.url,
            state: pull.state,
            head_sha: pull.head_ref_oid.filter(|sha| !sha.is_empty()),
        })
        .collect())
}

/// Every PR the repository has, open, merged or closed. It is read once for the whole repository, so
/// each workspace can show whether its work has merged without a request of its own.
pub async fn list(gh: Option<&Path>, repo: &Path, remote: &str) -> AppResult<Vec<BranchPull>> {
    let gh = gh.ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    validate_branch(remote)?;
    let repo_args = remotes::gh_repo_args(repo, remote).await;
    let mut args: Vec<&str> = vec![
        "pr",
        "list",
        "--state",
        "all",
        "--limit",
        LIST_LIMIT,
        "--json",
        "number,url,state,headRefName,headRefOid",
    ];
    args.extend(repo_args.iter().map(String::as_str));
    let output = github::run_gh_within(gh, repo, &args, PR_TIMEOUT).await?;
    if !output.status.success() {
        crate::error::bail!("{}", first_line(&output.stderr, "gh pr list failed"));
    }
    parse_list(&output.stdout)
}

/// Pushes `branch` and opens a PR into `base` from its commits.
pub async fn create(
    gh: Option<&Path>,
    worktree: &Path,
    branch: &str,
    base: &str,
    remote: &str,
) -> AppResult<PullRequestStatus> {
    let gh = gh.ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    validate_branch(branch)?;
    validate_branch(base)?;
    validate_branch(remote)?;
    // A branch with no commits beyond the base has nothing for a PR to show. Checked before the push,
    // so no empty branch is published, and so `gh` is not left to fail with its own message.
    // If the base's remote-tracking ref is missing, the check is skipped and gh decides as before.
    let ahead_range = format!("{remote}/{base}..{branch}");
    if let Ok(count) = git::cmd::run(worktree, &["rev-list", "--count", ahead_range.as_str()]).await {
        if count.trim() == "0" {
            crate::error::bail!(
                "{branch} has no commits beyond {remote}/{base} yet, so there is no PR to open. Commit your changes first, then create the PR."
            );
        }
    }
    // Push first, so `gh pr create` never has to ask about an unpushed branch.
    git::cmd::run(worktree, &["push", "--set-upstream", remote, branch]).await?;
    let repo = remotes::gh_repo_args(worktree, remote).await;
    let mut args: Vec<&str> = vec!["pr", "create", "--base", base, "--head", branch, "--fill"];
    args.extend(repo.iter().map(String::as_str));
    let output = github::run_gh_within(gh, worktree, &args, PR_TIMEOUT).await?;
    if !output.status.success() {
        crate::error::bail!("{}", first_line(&output.stderr, "gh pr create failed"));
    }
    status(Some(gh), worktree, branch, remote)
        .await?
        .ok_or_else(|| AppError::msg("the PR was created, but gh could not read it back"))
}

/// Moves a workspace onto a new branch at its current commit, bringing its commits and its
/// uncommitted edits along. Work done on the base branch itself cannot be a PR, because a PR needs a
/// branch of its own to start from. This gives it one. The branch it leaves is not touched, so
/// commits made there stay there. Only ever runs from an explicit user action.
pub async fn start_branch(worktree: &Path, branch: &str) -> AppResult<()> {
    validate_branch(branch)?;
    git::cmd::run(worktree, &["switch", "--create", branch]).await?;
    Ok(())
}

/// Stages every change in the worktree and commits it, so a PR opened afterwards includes the
/// agent's edits. Returns the new commit, or `None` when nothing was changed. Only ever runs from
/// an explicit user action, never automatically.
pub async fn commit_changes(worktree: &Path, message: &str) -> AppResult<Option<String>> {
    let message = message.trim();
    if message.is_empty() {
        crate::error::bail!("Write a commit message before committing the changes.");
    }
    if git::cmd::run(worktree, &["status", "--porcelain"]).await?.trim().is_empty() {
        return Ok(None);
    }
    git::cmd::run(worktree, &["add", "-A"]).await?;
    git::cmd::run(worktree, &["commit", "-m", message]).await?;
    let head = git::cmd::run(worktree, &["rev-parse", "HEAD"]).await?;
    Ok(Some(head.trim().to_string()))
}

/// Squash-merges the PR for `branch`. The check runs here as well as on GitHub, so the app
/// never asks for a merge it already knows will be refused.
pub async fn merge(gh: Option<&Path>, worktree: &Path, branch: &str, remote: &str) -> AppResult<PullRequestStatus> {
    let gh = gh.ok_or_else(|| AppError::msg(github::GH_MISSING))?;
    let current = status(Some(gh), worktree, branch, remote)
        .await?
        .ok_or_else(|| AppError::msg("This branch has no pull request yet."))?;
    if let Some(reason) = merge_blocker(&current) {
        crate::error::bail!("{reason}");
    }
    let number = current.number.to_string();
    let repo = remotes::gh_repo_args(worktree, remote).await;
    let mut args: Vec<&str> = vec!["pr", "merge", number.as_str(), "--squash"];
    args.extend(repo.iter().map(String::as_str));
    let output = github::run_gh_within(gh, worktree, &args, PR_TIMEOUT).await?;
    if !output.status.success() {
        crate::error::bail!("{}", first_line(&output.stderr, "gh pr merge failed"));
    }
    status(Some(gh), worktree, branch, remote)
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
pub async fn sync_with_base(worktree: &Path, branch: &str, base: &str, remote: &str) -> AppResult<SyncOutcome> {
    validate_branch(branch)?;
    validate_branch(base)?;
    validate_branch(remote)?;
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
                message: format!("Conflicts with {remote}/{base} are still open. Resolve them, then press Resolve again."),
            });
        }
        git::cmd::run(worktree, &["commit", "--no-edit"]).await?;
    } else {
        // Refuse to merge over uncommitted work. Git would carry it along into the merge commit.
        if !git::cmd::run(worktree, &["status", "--porcelain"]).await?.trim().is_empty() {
            crate::error::bail!("Commit or stash your changes before syncing with {base}.");
        }
        git::cmd::run(worktree, &["fetch", remote, base]).await?;
        let remote_base = format!("{remote}/{base}");
        let merge = git::cmd::raw(worktree, &["merge", "--no-edit", remote_base.as_str()]).await?;
        if !merge.status.success() {
            let conflicts = unmerged_files(worktree).await?;
            if conflicts.is_empty() {
                crate::error::bail!("{}", first_line(&merge.stderr, "git merge failed"));
            }
            return Ok(SyncOutcome {
                pushed: false,
                conflicts,
                message: format!("{remote}/{base} conflicts with this branch."),
            });
        }
    }

    git::cmd::run(worktree, &["push", remote, branch]).await?;
    Ok(SyncOutcome {
        pushed: true,
        conflicts: vec![],
        message: format!("Merged {remote}/{base} and pushed {branch}. Checks will run again."),
    })
}

async fn unmerged_files(worktree: &Path) -> AppResult<Vec<String>> {
    let text = git::cmd::run(worktree, &["diff", "--name-only", "--diff-filter=U"]).await?;
    Ok(text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect())
}

/// What a review covers and what the reviewer is told about the run. Each field is optional, so a
/// review that selects nothing is refused.
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ReviewContext {
    /// Commits on the branch since the base: the content of the PR.
    pub committed: bool,
    /// Edits that are not committed yet.
    pub uncommitted: bool,
    /// The commit list since the base.
    pub commit_log: bool,
    /// The run's goal, to judge the change against.
    pub goal: Option<String>,
    /// What the run's agents were asked and what each returned.
    pub handoffs: Option<String>,
    /// A command that verifies the change. The reviewer runs it and reports the result.
    pub test_command: Option<String>,
    /// Anything specific the reviewer should look for.
    pub focus: Option<String>,
}

/// Longest free text sent to the reviewer, so a long run cannot overflow the command line.
const REVIEW_TEXT_CHARS: usize = 8_000;

/// Trimmed, bounded text, or `None` when there is nothing to say.
fn clean(text: &Option<String>) -> Option<String> {
    let trimmed = text.as_deref()?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(REVIEW_TEXT_CHARS).collect())
}

/// The prompt a review starts with. It names the diffs to read, puts in the context the user chose,
/// and forbids edits, so the reviewer reports without changing the work.
pub fn review_prompt(base: &str, context: &ReviewContext) -> AppResult<String> {
    validate_branch(base)?;
    if !context.committed && !context.uncommitted {
        crate::error::bail!("Choose what to review: committed changes, uncommitted edits, or both.");
    }
    let mut out = format!(
        "Review the changes on this branch against {base}. Do not edit files, commit, or push.\n"
    );
    if let Some(goal) = clean(&context.goal) {
        out.push_str(&format!("\n## Goal\n{goal}\n"));
    }
    if let Some(handoffs) = clean(&context.handoffs) {
        out.push_str(&format!("\n## How this run got here\n{handoffs}\n"));
    }
    out.push_str("\n## What to read\n");
    if context.committed {
        out.push_str(&format!("- Committed changes: `git diff {base}...HEAD`\n"));
    }
    if context.uncommitted {
        out.push_str("- Uncommitted edits: `git status --short` and `git diff`. Read new files directly.\n");
    }
    if context.commit_log {
        out.push_str(&format!("- Commits: `git log --oneline {base}..HEAD`\n"));
    }
    if let Some(focus) = clean(&context.focus) {
        out.push_str(&format!("\n## Focus\n{focus}\n"));
    }
    if let Some(command) = clean(&context.test_command) {
        out.push_str(&format!("\n## Verify\nRun `{command}` and report the result.\n"));
    }
    out.push_str(
        "\n## Report\nList correctness bugs, risky changes and missing tests, most severe first, each with its file and line. \
         End with a verdict: approve, or changes requested.\n",
    );
    Ok(out)
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

    #[test]
    fn the_repository_list_names_each_pr_by_its_branch() {
        let json = br#"[
            {"number":4,"url":"https://github.com/o/r/pull/4","state":"OPEN","headRefName":"oc/a/changes","headRefOid":"abc"},
            {"number":2,"url":"https://github.com/o/r/pull/2","state":"MERGED","headRefName":"feature/x","headRefOid":""}
        ]"#;
        let pulls = parse_list(json).unwrap();
        assert_eq!(pulls.len(), 2);
        assert_eq!(pulls[0].branch, "oc/a/changes");
        assert_eq!(pulls[0].state, "OPEN");
        assert_eq!(pulls[0].head_sha.as_deref(), Some("abc"));
        assert_eq!(pulls[1].branch, "feature/x");
        assert_eq!(pulls[1].head_sha, None, "an empty head is no head");
    }

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
            head_sha: None,
        }
    }

    #[test]
    fn the_commit_at_the_tip_of_the_pr_is_read_and_an_empty_one_is_ignored() {
        let with_sha = br#"{"number":7,"url":"u","state":"MERGED","headRefOid":"abc123"}"#;
        assert_eq!(parse_pull(with_sha).unwrap().head_sha.as_deref(), Some("abc123"));
        let empty = br#"{"number":7,"url":"u","state":"OPEN","headRefOid":""}"#;
        assert_eq!(parse_pull(empty).unwrap().head_sha, None);
        let missing = br#"{"number":7,"url":"u","state":"OPEN"}"#;
        assert_eq!(parse_pull(missing).unwrap().head_sha, None);
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

    fn review(committed: bool, uncommitted: bool) -> ReviewContext {
        ReviewContext { committed, uncommitted, ..ReviewContext::default() }
    }

    #[test]
    fn a_review_names_the_diffs_and_forbids_edits() {
        let text = review_prompt("origin/main", &review(true, true)).unwrap();
        assert!(text.contains("against origin/main"));
        assert!(text.contains("Do not edit files"));
        assert!(text.contains("git diff origin/main...HEAD"));
        assert!(text.contains("git status --short"));
        assert!(!text.contains("## Goal"), "nothing was asked for, so nothing is added");
    }

    #[test]
    fn only_the_chosen_scope_is_read() {
        let committed_only = review_prompt("origin/main", &review(true, false)).unwrap();
        assert!(committed_only.contains("git diff origin/main...HEAD"));
        assert!(!committed_only.contains("git status --short"));

        let uncommitted_only = review_prompt("origin/main", &review(false, true)).unwrap();
        assert!(!uncommitted_only.contains("...HEAD"));
        assert!(uncommitted_only.contains("git status --short"));
    }

    #[test]
    fn the_chosen_context_is_included_and_blank_text_is_not() {
        let context = ReviewContext {
            committed: true,
            commit_log: true,
            goal: Some("  add dark mode  ".into()),
            handoffs: Some("CEO → Senior Dev: wrote the toggle".into()),
            test_command: Some("npm test".into()),
            focus: Some("   ".into()),
            ..ReviewContext::default()
        };
        let text = review_prompt("origin/main", &context).unwrap();
        assert!(text.contains("## Goal\nadd dark mode\n"));
        assert!(text.contains("## How this run got here"));
        assert!(text.contains("git log --oneline origin/main..HEAD"));
        assert!(text.contains("Run `npm test` and report the result."));
        assert!(!text.contains("## Focus"), "a blank focus adds nothing");
    }

    #[test]
    fn a_review_with_nothing_selected_is_refused_and_long_text_is_bounded() {
        assert!(review_prompt("origin/main", &review(false, false)).is_err());
        assert!(review_prompt("--force", &review(true, false)).is_err());

        let long = ReviewContext { committed: true, focus: Some("x".repeat(50_000)), ..ReviewContext::default() };
        let text = review_prompt("origin/main", &long).unwrap();
        assert!(text.len() < 10_000, "free text is capped, got {} bytes", text.len());
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
