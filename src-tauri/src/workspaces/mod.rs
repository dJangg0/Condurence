//! Creating a workspace the way Conductor does. One click makes an isolated copy of the repository
//! on a temporary branch, named after a League of Legends champion, that starts from the base
//! branch in the repository settings. The user merges it into their branches by hand later.

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;

use crate::error::AppResult;
use crate::git::cmd;
use crate::git::diff_engine::parse_status_porcelain_v2;
use crate::git::locks::RepoLocks;
use crate::git::worktree::{self, WorktreeInfo};
use crate::git::{repo_root, WORKTREES_DIR};
use crate::pulls::{self, PullRequestStatus};
use crate::settings::{valid_name, RepoGitSettings};

/// Champion names as lowercase git-safe slugs. Each is used at most once per repository, and a
/// name that is still in use is skipped.
pub const CHAMPIONS: &[&str] = &[
    "aatrox", "ahri", "akali", "akshan", "alistar", "ambessa", "amumu", "anivia", "annie", "aphelios", "ashe",
    "aurelion-sol", "azir", "bard", "belveth", "blitzcrank", "brand", "braum", "briar", "caitlyn", "camille",
    "cassiopeia", "cho-gath", "corki", "darius", "diana", "dr-mundo", "draven", "ekko", "elise", "evelynn",
    "ezreal", "fiddlesticks", "fiora", "fizz", "galio", "gangplank", "garen", "gnar", "gragas", "graves", "gwen",
    "hecarim", "heimerdinger", "hwei", "illaoi", "irelia", "ivern", "janna", "jarvan-iv", "jax", "jayce", "jhin",
    "jinx", "kai-sa", "kalista", "karma", "karthus", "kassadin", "katarina", "kayle", "kayn", "kennen", "kha-zix",
    "kindred", "kled", "kog-maw", "ksante", "leblanc", "lee-sin", "leona", "lillia", "lissandra", "lucian", "lulu",
    "lux", "malphite", "malzahar", "maokai", "master-yi", "mel", "milio", "miss-fortune", "mordekaiser", "morgana",
    "naafiri", "nami", "nasus", "nautilus", "neeko", "nidalee", "nilah", "nocturne", "nunu", "olaf", "orianna",
    "ornn", "pantheon", "poppy", "pyke", "qiyana", "quinn", "rakan", "rammus", "rek-sai", "rell", "renata-glasc",
    "renekton", "rengar", "riven", "rumble", "ryze", "samira", "sejuani", "senna", "seraphine", "sett", "shaco",
    "shen", "shyvana", "singed", "sion", "sivir", "skarner", "smolder", "sona", "soraka", "swain", "sylas",
    "syndra", "tahm-kench", "taliyah", "talon", "taric", "teemo", "thresh", "tristana", "trundle", "tryndamere",
    "twisted-fate", "twitch", "udyr", "urgot", "varus", "vayne", "veigar", "velkoz", "vex", "vi", "viego", "viktor",
    "vladimir", "volibear", "warwick", "wukong", "xayah", "xerath", "xin-zhao", "yasuo", "yone", "yorick", "yuumi",
    "zac", "zed", "zeri", "ziggs", "zilean", "zoe", "zyra",
];

/// A champion name that is not in `taken`, starting from `seed` so names do not always come in the
/// same order. Once every name is taken, a numbered name such as `ahri-2` is used.
pub fn pick_free_name(taken: &HashSet<String>, seed: u64) -> String {
    let count = CHAMPIONS.len();
    let start = (seed % count as u64) as usize;
    for offset in 0..count {
        let name = CHAMPIONS[(start + offset) % count];
        if !taken.contains(name) {
            return name.to_string();
        }
    }
    let base = CHAMPIONS[start];
    (2..)
        .map(|number| format!("{base}-{number}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unbounded search always finds a free name")
}

/// Creates a workspace from the repository settings: a new branch and folder named after a free
/// champion, starting from the base. The base is fetched first. Offline, the copy from the last
/// fetch is used, and it has to exist.
pub async fn create_workspace(
    locks: &RepoLocks,
    repo_path: &Path,
    settings: &RepoGitSettings,
    seed: u64,
) -> AppResult<WorktreeInfo> {
    let root = repo_root(repo_path).await?;
    let taken = taken_names(&root).await?;
    let name = pick_free_name(&taken, seed);

    let base_branch = settings.base_branch();
    let _ = cmd::raw(&root, &["fetch", "--quiet", "--no-tags", settings.remote.as_str(), base_branch.as_str()]).await;
    let tracking = format!("refs/remotes/{}", settings.base_ref);
    let has_base = cmd::raw(&root, &["rev-parse", "--verify", "--quiet", tracking.as_str()])
        .await?
        .status
        .success();
    if !has_base {
        crate::error::bail!(
            "{} is not available. Fetch it from {}, or change the base in the repository settings.",
            settings.base_ref,
            settings.remote
        );
    }

    let created = worktree::create_worktree(locks, &root, &name, &name, Some(&settings.base_ref)).await?;
    if settings.set_upstream_on_push {
        set_upstream_on_push(&root, Path::new(&created.path)).await?;
    }
    Ok(created)
}

/// What archiving a workspace would do, for the confirmation shown before it happens.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePreview {
    /// Files with changes that are not committed, tracked or not. Archiving removes the folder, so
    /// these are discarded.
    pub uncommitted: Vec<String>,
    /// Commits on the branch that the remote has no copy of. Archiving keeps them on the branch.
    pub unpushed: u32,
    /// The branch's PR, when GitHub could be asked and the branch has one.
    pub pr: Option<PullRequestStatus>,
    /// The PR is merged and the branch holds nothing newer than what was merged. Only then is it
    /// certain that deleting the branch loses nothing, even after a squash merge that git cannot
    /// see as merged.
    pub delivered: bool,
}

/// What happened to the branch when a workspace was archived.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOutcome {
    pub branch_deleted: bool,
    /// Why the branch was kept although the repository settings ask to delete it. `None` when it
    /// was deleted, or when the settings say to keep it.
    pub branch_note: Option<String>,
}

/// The PR of `branch` and whether it is delivered (see `ArchivePreview::delivered`). Without `gh`,
/// or when GitHub cannot be asked, there is no PR and nothing counts as delivered.
async fn delivery(gh: Option<&Path>, worktree: &Path, branch: &str, remote: &str) -> (Option<PullRequestStatus>, bool) {
    let Some(gh) = gh else { return (None, false) };
    if !worktree::is_valid_branch(branch) || !valid_name(remote) {
        return (None, false);
    }
    let pr = pulls::status(Some(gh), worktree, branch, remote).await.ok().flatten();
    let tip = cmd::run(worktree, &["rev-parse", "HEAD"]).await.ok().map(|sha| sha.trim().to_string());
    let delivered = pr
        .as_ref()
        .is_some_and(|pr| pr.state == "MERGED" && pr.head_sha.is_some() && pr.head_sha == tip);
    (pr, delivered)
}

/// Reads what archiving `worktree` would discard and keep. Nothing is changed.
pub async fn archive_preview(gh: Option<&Path>, worktree: &Path, branch: Option<&str>, remote: &str) -> AppResult<ArchivePreview> {
    let status = cmd::run(worktree, &["status", "--porcelain=v2", "-z", "--untracked-files=all"]).await?;
    let uncommitted = parse_status_porcelain_v2(&status).into_iter().map(|entry| entry.path).collect();

    let unpushed = if valid_name(remote) {
        let remotes = format!("--remotes={remote}");
        cmd::run(worktree, &["rev-list", "--count", "HEAD", "--not", remotes.as_str()])
            .await
            .ok()
            .and_then(|count| count.trim().parse().ok())
            .unwrap_or(0)
    } else {
        0
    };

    let (pr, delivered) = match branch {
        Some(branch) => delivery(gh, worktree, branch, remote).await,
        None => (None, false),
    };
    Ok(ArchivePreview { uncommitted, unpushed, pr, delivered })
}

/// Archives a workspace: removes its folder, and deletes its branch only when `delete_branch` asks
/// for it and that loses nothing. It only ever runs from the user's own request. A PR being created
/// or merged does not archive a workspace.
///
/// A workspace with uncommitted changes is refused unless `discard_changes` says they may go. The
/// branch is deleted with `-d` (refuses work that is not merged) or, when GitHub reports the PR
/// merged and the branch has nothing newer than that, with `-D`, since a squash merge looks
/// unmerged to git. A branch that stays is explained in the outcome. Detached workspaces have no
/// branch (`branch` is `None`).
pub async fn archive_workspace(
    locks: &RepoLocks,
    gh: Option<&Path>,
    repo_path: &Path,
    branch: Option<&str>,
    folder: &str,
    remote: &str,
    delete_branch: bool,
    discard_changes: bool,
) -> AppResult<ArchiveOutcome> {
    if !worktree::is_valid_folder(folder) {
        crate::error::bail!("'{folder}' is not a valid workspace folder name");
    }
    if let Some(branch) = branch {
        if !worktree::is_valid_branch(branch) {
            crate::error::bail!("'{branch}' is not a valid branch name");
        }
    }
    let root = repo_root(repo_path).await?;
    let target = root.join(WORKTREES_DIR).join(folder);
    if !target.is_dir() {
        crate::error::bail!("'{folder}' is not a workspace of this repository");
    }

    // Ask GitHub while the folder still exists, since `gh` runs inside it. Only a branch that is
    // about to be deleted needs the answer.
    let delivered = match branch {
        Some(branch) if delete_branch => delivery(gh, &target, branch, remote).await.1,
        _ => false,
    };

    let _permit = locks.acquire(&root).await;
    if !discard_changes {
        let status = cmd::run(&target, &["status", "--porcelain=v2", "-z", "--untracked-files=all"]).await?;
        let pending = parse_status_porcelain_v2(&status).len();
        if pending > 0 {
            crate::error::bail!(
                "{folder} has {pending} uncommitted file{}. Archiving would discard them, so nothing was changed.",
                if pending == 1 { "" } else { "s" }
            );
        }
    }
    let target_str = target.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if discard_changes {
        args.push("--force");
    }
    args.push(target_str.as_str());
    cmd::run(&root, &args).await?;

    let Some(branch) = branch.filter(|_| delete_branch) else {
        return Ok(ArchiveOutcome { branch_deleted: false, branch_note: None });
    };
    let flag = if delivered { "-D" } else { "-d" };
    let output = cmd::raw(&root, &["branch", flag, branch]).await?;
    if output.status.success() {
        return Ok(ArchiveOutcome { branch_deleted: true, branch_note: None });
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let note = if !delivered && stderr.contains("not fully merged") {
        "it has commits that are not merged into another branch".to_string()
    } else {
        stderr.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("git would not delete it").to_string()
    };
    Ok(ArchiveOutcome { branch_deleted: false, branch_note: Some(note) })
}

/// Names already in use: local branches and workspace folders, so a new name never collides.
async fn taken_names(root: &Path) -> AppResult<HashSet<String>> {
    let mut taken = HashSet::new();
    let branches = cmd::run(root, &["for-each-ref", "--format=%(refname:short)", "refs/heads/"]).await?;
    taken.extend(branches.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string));
    if let Ok(entries) = std::fs::read_dir(root.join(WORKTREES_DIR)) {
        taken.extend(entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()));
    }
    Ok(taken)
}

/// Makes a plain `git push` in this workspace set its upstream. Per-worktree config needs the
/// repository's `extensions.worktreeConfig`, which only changes how git reads config here.
async fn set_upstream_on_push(root: &Path, workspace: &Path) -> AppResult<()> {
    cmd::run(root, &["config", "extensions.worktreeConfig", "true"]).await?;
    cmd::run(workspace, &["config", "--worktree", "push.autoSetupRemote", "true"]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn champion_names_are_unique_git_safe_slugs() {
        let unique: HashSet<&str> = CHAMPIONS.iter().copied().collect();
        assert_eq!(unique.len(), CHAMPIONS.len(), "duplicate champion name");
        for name in CHAMPIONS {
            assert!(
                !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                    && !name.starts_with('-')
                    && !name.ends_with('-'),
                "'{name}' is not a plain slug"
            );
        }
    }

    #[test]
    fn a_free_champion_is_picked_from_the_seed_onwards() {
        let seed = CHAMPIONS.len() as u64 + 3;
        assert_eq!(pick_free_name(&taken(&[]), seed), CHAMPIONS[3]);
    }

    #[test]
    fn names_in_use_are_skipped() {
        let seed = 0;
        assert_eq!(pick_free_name(&taken(&[CHAMPIONS[0]]), seed), CHAMPIONS[1]);
        assert_eq!(pick_free_name(&taken(&[CHAMPIONS[0], CHAMPIONS[1]]), seed), CHAMPIONS[2]);
    }

    #[test]
    fn once_every_champion_is_taken_a_numbered_name_is_used() {
        let every: HashSet<String> = CHAMPIONS.iter().map(|name| name.to_string()).collect();
        assert_eq!(pick_free_name(&every, 0), format!("{}-2", CHAMPIONS[0]));

        let mut with_first_suffix = every.clone();
        with_first_suffix.insert(format!("{}-2", CHAMPIONS[0]));
        assert_eq!(pick_free_name(&with_first_suffix, 0), format!("{}-3", CHAMPIONS[0]));
    }
}
