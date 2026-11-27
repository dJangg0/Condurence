//! Browsing a workspace's files: every tracked file plus untracked files that are not ignored, and
//! the text of one of them.
//!
//! Git decides what can be seen. A file that git ignores, such as `.env` or build output, is neither
//! listed nor readable, and a symlink that leads outside the workspace is refused. The UI never
//! hands over a path it did not get from the listing, but the check is made here as well, because
//! the command is reachable from the webview.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use super::cmd;
use crate::error::{AppError, AppResult};

/// Files listed at most. A repository larger than this is shown truncated rather than freezing the tree.
pub const MAX_LISTED_FILES: usize = 20_000;
/// Bytes of a file returned at most. The rest is cut and flagged `truncated`.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// A file is binary when its first bytes hold a NUL, the check git itself uses.
const BINARY_SNIFF_BYTES: usize = 8000;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileListing {
    /// Workspace-relative paths with `/` separators, sorted.
    pub files: Vec<String>,
    /// More files exist than `MAX_LISTED_FILES`.
    pub truncated: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub path: String,
    /// Lossy UTF-8 text. Empty for a binary file.
    pub content: String,
    pub binary: bool,
    /// The file is longer than `MAX_FILE_BYTES` and `content` is its beginning.
    pub truncated: bool,
    /// Size on disk in bytes.
    pub size: u64,
}

/// The files of the workspace as they are now: tracked files that still exist, plus untracked files
/// that are not ignored.
pub async fn list_files(worktree: &Path) -> AppResult<FileListing> {
    let output = cmd::run(worktree, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"]).await?;
    let root = worktree.to_path_buf();
    // Stat every entry off the async threads: a large repository has thousands.
    tokio::task::spawn_blocking(move || existing_files(&root, &output))
        .await
        .map_err(|e| AppError::msg(format!("listing files failed: {e}")))
}

/// Keeps the entries that are files on disk. The index still names a tracked file that was deleted
/// from the working tree, and a submodule or nested repository shows up as a directory.
fn existing_files(root: &Path, nul_separated: &str) -> FileListing {
    let mut files = BTreeSet::new();
    for entry in nul_separated.split('\0').filter(|entry| !entry.is_empty()) {
        if entry.ends_with('/') {
            continue;
        }
        match std::fs::symlink_metadata(root.join(entry)) {
            Ok(meta) if !meta.is_dir() => {
                files.insert(entry.to_string());
            }
            _ => {}
        }
    }
    let truncated = files.len() > MAX_LISTED_FILES;
    FileListing { files: files.into_iter().take(MAX_LISTED_FILES).collect(), truncated }
}

/// Reads one listed file. Fails for a path git does not list, one outside the workspace, or a
/// directory.
pub async fn read_file(worktree: &Path, relative: &str) -> AppResult<FileContent> {
    let relative = checked_relative(relative)?;
    // Git has the last word on whether the file is part of the workspace: an ignored path is not.
    let pathspec = format!(":(literal){relative}");
    let listed = cmd::run(
        worktree,
        &["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", &pathspec],
    )
    .await?;
    if listed.split('\0').all(|entry| entry != relative) {
        return Err(AppError::msg(format!("{relative} is not a file of this workspace")));
    }

    let root = worktree.to_path_buf();
    let relative = relative.to_string();
    tokio::task::spawn_blocking(move || read_inside(&root, &relative))
        .await
        .map_err(|e| AppError::msg(format!("reading the file failed: {e}")))?
}

/// A workspace-relative path with no way out: not empty, not absolute, no `..`, not inside `.git`.
fn checked_relative(path: &str) -> AppResult<&str> {
    let candidate = Path::new(path);
    let mut components = candidate.components().peekable();
    let escapes = path.is_empty()
        || candidate.is_absolute()
        || components.peek().is_none()
        || candidate.components().any(|component| !matches!(component, Component::Normal(_)))
        || matches!(components.peek(), Some(Component::Normal(first)) if *first == ".git");
    if escapes {
        return Err(AppError::msg(format!("'{path}' is not a path inside the workspace")));
    }
    Ok(path)
}

fn read_inside(root: &Path, relative: &str) -> AppResult<FileContent> {
    let canonical_root = root.canonicalize()?;
    let target: PathBuf = canonical_root
        .join(relative)
        .canonicalize()
        .map_err(|_| AppError::msg(format!("{relative} no longer exists in this workspace")))?;
    // Resolving symlinks first means a link to /etc/passwd cannot pass as a workspace file.
    if !target.starts_with(&canonical_root) {
        return Err(AppError::msg(format!("{relative} points outside the workspace")));
    }
    let meta = std::fs::metadata(&target)?;
    if !meta.is_file() {
        return Err(AppError::msg(format!("{relative} is not a file")));
    }

    let size = meta.len();
    let mut bytes = Vec::new();
    std::fs::File::open(&target)?.take(MAX_FILE_BYTES).read_to_end(&mut bytes)?;
    let binary = bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0);
    Ok(FileContent {
        path: relative.to_string(),
        content: if binary { String::new() } else { String::from_utf8_lossy(&bytes).into_owned() },
        binary,
        truncated: size > MAX_FILE_BYTES,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_that_leave_the_workspace_are_refused() {
        for bad in ["", "/etc/passwd", "../outside", "src/../../outside", ".git/config", "./a"] {
            assert!(checked_relative(bad).is_err(), "{bad:?} should be refused");
        }
        for good in ["README.md", "src/lib/a.ts", "dir with space/file.txt", ".github/workflows/ci.yml", ".gitignore"] {
            assert_eq!(checked_relative(good).unwrap(), good);
        }
    }

    #[test]
    fn the_listing_drops_directories_and_deleted_files_and_removes_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("nested-repo")).unwrap();
        std::fs::write(tmp.path().join("b.txt"), "b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();

        // `gone.txt` is still in the index but not on disk, `nested-repo` is a directory, and the
        // conflicted `a.txt` appears once per stage.
        let listing = existing_files(tmp.path(), "b.txt\0a.txt\0a.txt\0gone.txt\0nested-repo\0nested-repo/\0");
        assert_eq!(listing.files, ["a.txt", "b.txt"]);
        assert!(!listing.truncated);
    }
}
