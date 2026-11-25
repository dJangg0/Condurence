//! Structured diffs and status snapshots.
//!
//! `git diff` output is parsed into hunks of typed lines so the UI can render a
//! side-by-side review without re-parsing text. Untracked files are synthesized as
//! all-added hunks, because `git diff` never reports them.

use std::path::Path;

use serde::Serialize;

use super::cmd;
use crate::error::AppResult;

/// The well-known empty tree, used as the diff base in repositories without commits.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Untracked files above this size are reported as binary-like instead of inlined.
const MAX_UNTRACKED_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub content: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// One entry from `git status --porcelain=v2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub status: FileStatus,
}

/// Parses a `git diff` text stream. Hunk bodies are consumed by line counts from the
/// hunk header, so a deleted line that happens to start with `-- ` is never mistaken for
/// a `---` file header.
pub fn parse_unified_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("diff --git ") else {
            continue;
        };
        let mut file = FileDiff {
            path: header_fallback_path(rest),
            old_path: None,
            status: FileStatus::Modified,
            additions: 0,
            deletions: 0,
            binary: false,
            hunks: Vec::new(),
        };

        // Extended header lines run until the first hunk or the next file.
        while let Some(&next) = lines.peek() {
            if next.starts_with("diff --git ") || next.starts_with("@@ ") {
                break;
            }
            let header_line = lines.next().unwrap_or_default();
            if header_line.starts_with("new file mode") {
                file.status = FileStatus::Added;
            } else if header_line.starts_with("deleted file mode") {
                file.status = FileStatus::Deleted;
            } else if let Some(from) = header_line.strip_prefix("rename from ") {
                file.status = FileStatus::Renamed;
                file.old_path = Some(from.to_string());
            } else if let Some(to) = header_line.strip_prefix("rename to ") {
                file.path = to.to_string();
            } else if let Some(path) = header_line.strip_prefix("+++ b/") {
                file.path = path.to_string();
            } else if let Some(path) = header_line.strip_prefix("--- a/") {
                if file.old_path.is_none() {
                    file.old_path = Some(path.to_string());
                }
            } else if header_line.starts_with("Binary files") {
                file.binary = true;
            }
        }
        if file.status != FileStatus::Renamed {
            file.old_path = None;
        }

        // Hunks, bounded by their declared line counts.
        while let Some(&next) = lines.peek() {
            if next.starts_with("diff --git ") {
                break;
            }
            let Some(header) = lines.next() else { break };
            if !header.starts_with("@@ ") {
                continue;
            }
            let Some((old_start, old_lines, new_start, new_lines)) = parse_hunk_header(header) else {
                continue;
            };
            let mut hunk = Hunk {
                header: header.to_string(),
                old_start,
                old_lines,
                new_start,
                new_lines,
                lines: Vec::new(),
            };
            let mut old_no = old_start;
            let mut new_no = new_start;
            let mut old_left = old_lines;
            let mut new_left = new_lines;

            while old_left > 0 || new_left > 0 {
                let Some(body) = lines.next() else { break };
                if let Some(content) = body.strip_prefix('+') {
                    hunk.lines.push(DiffLine {
                        kind: LineKind::Add,
                        old_line: None,
                        new_line: Some(new_no),
                        content: content.to_string(),
                    });
                    new_no += 1;
                    new_left = new_left.saturating_sub(1);
                    file.additions += 1;
                } else if let Some(content) = body.strip_prefix('-') {
                    hunk.lines.push(DiffLine {
                        kind: LineKind::Del,
                        old_line: Some(old_no),
                        new_line: None,
                        content: content.to_string(),
                    });
                    old_no += 1;
                    old_left = old_left.saturating_sub(1);
                    file.deletions += 1;
                } else if body.starts_with('\\') {
                    // "\ No newline at end of file" marker; not a content line.
                    continue;
                } else {
                    let content = body.strip_prefix(' ').unwrap_or(body);
                    hunk.lines.push(DiffLine {
                        kind: LineKind::Context,
                        old_line: Some(old_no),
                        new_line: Some(new_no),
                        content: content.to_string(),
                    });
                    old_no += 1;
                    new_no += 1;
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                }
            }
            file.hunks.push(hunk);
        }

        files.push(file);
    }
    files
}

/// `a/<path> b/<path>` header fallback used when no `+++` line exists (binary or pure rename).
fn header_fallback_path(rest: &str) -> String {
    match rest.rsplit_once(" b/") {
        Some((_, new)) => new.to_string(),
        None => rest.to_string(),
    }
}

/// Parses `@@ -a[,b] +c[,d] @@`. A missing count means 1, per the unified diff format.
fn parse_hunk_header(header: &str) -> Option<(u32, u32, u32, u32)> {
    let body = header.strip_prefix("@@ ")?;
    let (ranges, _) = body.split_once(" @@")?;
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;
    Some((old_start, old_lines, new_start, new_lines))
}

fn parse_range(range: &str) -> Option<(u32, u32)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Parses `git status --porcelain=v2 -z`. Renames consume the following NUL-separated
/// record (the original path), so records are walked with an explicit cursor.
pub fn parse_status_porcelain_v2(output: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut records = output.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        let kind = record.chars().next().unwrap_or(' ');
        match kind {
            '1' => {
                if let Some(path) = record.splitn(9, ' ').nth(8) {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        status: FileStatus::Modified,
                    });
                }
            }
            '2' => {
                if let Some(path) = record.splitn(10, ' ').nth(9) {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        status: FileStatus::Renamed,
                    });
                }
                let _original = records.next();
            }
            'u' => {
                if let Some(path) = record.splitn(11, ' ').nth(10) {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        status: FileStatus::Conflicted,
                    });
                }
            }
            '?' => {
                if let Some(path) = record.strip_prefix("? ") {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        status: FileStatus::Untracked,
                    });
                }
            }
            _ => {}
        }
    }
    entries
}

/// Counts changed files without building diffs. Used for the dirty badge.
pub fn changed_file_count(worktree: &Path) -> AppResult<usize> {
    let output = cmd::run_sync(
        worktree,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
        true,
    )?;
    Ok(parse_status_porcelain_v2(&output).len())
}

/// Full review payload for a worktree: tracked changes against HEAD plus synthesized
/// diffs for untracked files.
pub async fn worktree_diff(worktree: &Path) -> AppResult<Vec<FileDiff>> {
    let has_head = cmd::raw(worktree, &["rev-parse", "--verify", "-q", "HEAD"])
        .await?
        .status
        .success();
    let base = if has_head { "HEAD" } else { EMPTY_TREE };

    let text = cmd::run(
        worktree,
        &["diff", "--no-color", "--no-ext-diff", "--unified=3", "-M", base],
    )
    .await?;
    let mut files = parse_unified_diff(&text);

    let status = cmd::run(
        worktree,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    )
    .await?;
    for entry in parse_status_porcelain_v2(&status) {
        if entry.status != FileStatus::Untracked || files.iter().any(|f| f.path == entry.path) {
            continue;
        }
        files.push(synthesize_untracked(worktree, &entry.path));
    }
    Ok(files)
}

fn synthesize_untracked(worktree: &Path, relative: &str) -> FileDiff {
    let full = worktree.join(relative);
    let mut file = FileDiff {
        path: relative.to_string(),
        old_path: None,
        status: FileStatus::Untracked,
        additions: 0,
        deletions: 0,
        binary: false,
        hunks: Vec::new(),
    };
    let metadata = std::fs::metadata(&full);
    let bytes = match metadata {
        Ok(meta) if meta.len() <= MAX_UNTRACKED_BYTES => std::fs::read(&full).ok(),
        _ => None,
    };
    let Some(bytes) = bytes else {
        file.binary = true;
        return file;
    };
    if bytes.contains(&0) {
        file.binary = true;
        return file;
    }

    let text = String::from_utf8_lossy(&bytes);
    let content_lines: Vec<&str> = text.lines().collect();
    if content_lines.is_empty() {
        return file;
    }
    let count = content_lines.len() as u32;
    file.additions = count;
    file.hunks.push(Hunk {
        header: format!("@@ -0,0 +1,{count} @@"),
        old_start: 0,
        old_lines: 0,
        new_start: 1,
        new_lines: count,
        lines: content_lines
            .iter()
            .enumerate()
            .map(|(i, line)| DiffLine {
                kind: LineKind::Add,
                old_line: None,
                new_line: Some(i as u32 + 1),
                content: (*line).to_string(),
            })
            .collect(),
    });
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn a() {}
-fn b() {}
+fn b() { 1 }
+fn c() {}
 // end
diff --git a/old.txt b/new.txt
similarity index 90%
rename from old.txt
rename to new.txt
diff --git a/gone.md b/gone.md
deleted file mode 100644
--- a/gone.md
+++ /dev/null
@@ -1,2 +0,0 @@
-bye
-bye again
diff --git a/img.png b/img.png
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_modified_renamed_deleted_and_binary_files() {
        let files = parse_unified_diff(SAMPLE);
        assert_eq!(files.len(), 4);

        let lib = &files[0];
        assert_eq!(lib.path, "src/lib.rs");
        assert_eq!(lib.status, FileStatus::Modified);
        assert_eq!((lib.additions, lib.deletions), (2, 1));
        // Layout: context(1/1), del(old 2), add(new 2), add(new 3), context(3/4).
        assert_eq!(lib.hunks[0].lines[1].kind, LineKind::Del);
        assert_eq!(lib.hunks[0].lines[1].old_line, Some(2));
        assert_eq!(lib.hunks[0].lines[2].kind, LineKind::Add);
        assert_eq!(lib.hunks[0].lines[2].new_line, Some(2));
        assert_eq!(lib.hunks[0].lines[3].new_line, Some(3));
        assert_eq!(lib.hunks[0].lines[4].old_line, Some(3));
        assert_eq!(lib.hunks[0].lines[4].new_line, Some(4));

        let renamed = &files[1];
        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some("old.txt"));
        assert_eq!(renamed.path, "new.txt");

        let deleted = &files[2];
        assert_eq!(deleted.status, FileStatus::Deleted);
        assert_eq!(deleted.deletions, 2);

        assert!(files[3].binary);
    }

    #[test]
    fn deleted_line_that_looks_like_header_is_content() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,1 @@\n--- not a header\n kept\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files[0].deletions, 1);
        assert_eq!(files[0].hunks[0].lines[0].content, "-- not a header");
    }

    #[test]
    fn parses_status_including_rename_and_untracked() {
        let raw = "1 .M N... 100644 100644 100644 aaa bbb src/a.rs\0\
                   2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0\
                   ? scratch/notes.md\0";
        let entries = parse_status_porcelain_v2(raw);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "src/a.rs");
        assert_eq!(entries[1].path, "new.rs");
        assert_eq!(entries[1].status, FileStatus::Renamed);
        assert_eq!(entries[2].status, FileStatus::Untracked);
    }

    #[test]
    fn hunk_header_counts_default_to_one() {
        assert_eq!(parse_hunk_header("@@ -5 +7 @@"), Some((5, 1, 7, 1)));
        assert_eq!(parse_hunk_header("@@ -0,0 +1,3 @@ fn x"), Some((0, 0, 1, 3)));
    }
}
