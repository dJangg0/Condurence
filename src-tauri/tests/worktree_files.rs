//! The All files tab's backend, on a real repository: what is listed, what can be read, and what is
//! refused. Git decides visibility, so ignored files stay out of both.

use std::path::Path;
use std::process::Command;

use condurence_lib::git::files::{list_files, read_file, MAX_FILE_BYTES};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

fn write(repo: &Path, relative: &str, bytes: &[u8]) {
    let path = repo.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Condurence Test"]);
    write(dir, ".gitignore", b"secrets.env\ntarget/\n");
    write(dir, "README.md", b"# demo\n");
    write(dir, "src/main.rs", b"fn main() {}\n");
    write(dir, "gone.txt", b"to be deleted\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
    tmp
}

#[tokio::test]
async fn the_listing_is_tracked_and_untracked_files_without_ignored_or_deleted_ones() {
    let tmp = repo();
    let dir = tmp.path();
    write(dir, "notes/todo.md", b"- ship\n"); // untracked, not ignored
    write(dir, "secrets.env", b"TOKEN=abc\n"); // ignored
    write(dir, "target/debug/app", b"\x7fELF"); // ignored directory
    std::fs::remove_file(dir.join("gone.txt")).unwrap(); // still in the index

    let listing = list_files(dir).await.expect("listing");
    assert_eq!(listing.files, [".gitignore", "README.md", "notes/todo.md", "src/main.rs"]);
    assert!(!listing.truncated);
}

#[tokio::test]
async fn a_listed_file_is_read_and_an_unlisted_or_escaping_one_is_refused() {
    let tmp = repo();
    let dir = tmp.path();
    write(dir, "secrets.env", b"TOKEN=abc\n");

    let readme = read_file(dir, "README.md").await.expect("read a tracked file");
    assert_eq!(readme.content, "# demo\n");
    assert!(!readme.binary && !readme.truncated);
    assert_eq!(readme.size, 7);

    // An untracked file counts as part of the workspace.
    write(dir, "notes/todo.md", b"- ship\n");
    assert_eq!(read_file(dir, "notes/todo.md").await.unwrap().content, "- ship\n");

    assert!(read_file(dir, "secrets.env").await.is_err(), "ignored files are not readable");
    assert!(read_file(dir, "../outside.txt").await.is_err());
    assert!(read_file(dir, "/etc/passwd").await.is_err());
    assert!(read_file(dir, ".git/config").await.is_err());
    assert!(read_file(dir, "src").await.is_err(), "a directory is not a file");
    assert!(read_file(dir, "missing.txt").await.is_err());

    std::fs::remove_file(dir.join("gone.txt")).unwrap();
    assert!(read_file(dir, "gone.txt").await.is_err(), "a file deleted from disk cannot be read");
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_out_of_the_workspace_is_refused_and_one_inside_is_followed() {
    let tmp = repo();
    let dir = tmp.path();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("passwd"), "root:x:0:0\n").unwrap();

    std::os::unix::fs::symlink(outside.path().join("passwd"), dir.join("leak")).unwrap();
    std::os::unix::fs::symlink(dir.join("README.md"), dir.join("readme-link")).unwrap();
    git(dir, &["add", "."]);

    let error = read_file(dir, "leak").await.expect_err("a link out of the workspace");
    assert!(error.to_string().contains("outside the workspace"), "{error}");
    assert_eq!(read_file(dir, "readme-link").await.unwrap().content, "# demo\n");
}

#[tokio::test]
async fn binary_and_oversized_files_are_flagged_instead_of_inlined() {
    let tmp = repo();
    let dir = tmp.path();
    write(dir, "logo.bin", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
    let binary = read_file(dir, "logo.bin").await.expect("binary file");
    assert!(binary.binary);
    assert!(binary.content.is_empty());

    let big = vec![b'a'; MAX_FILE_BYTES as usize + 10];
    write(dir, "big.txt", &big);
    let cut = read_file(dir, "big.txt").await.expect("oversized file");
    assert!(cut.truncated);
    assert_eq!(cut.content.len() as u64, MAX_FILE_BYTES);
    assert_eq!(cut.size, MAX_FILE_BYTES + 10);
}
