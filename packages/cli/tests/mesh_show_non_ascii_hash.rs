//! Reproduction test for the byte-index slice panic in `wiki mesh show`.
//!
//! `manage.rs:128` uses `&anchor.content_hash[..8.min(anchor.content_hash.len())]`
//! to truncate the stored hash for display. When `content_hash` contains multi-byte
//! UTF-8 characters (e.g. emoji), byte 8 lands in the middle of a character,
//! causing a panic at runtime. The fix should use char-safe truncation
//! (`content_hash.chars().take(8).collect()`).
//!
//! **This test MUST FAIL against current unfixed code** — the child `wiki` binary
//! panics with a byte-index-out-of-bounds error, the parent assertion catches the
//! non-zero exit, and the test runner reports a failure.
//!
//! Once the fix is applied (char-safe truncation), the binary produces exit 0 and
//! the assertion passes.

use std::path::Path;
use std::process::{Command, Output};

fn git(workdir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

fn wiki(cwd: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run wiki")
}

/// When `content_hash` contains multi-byte UTF-8 characters, the byte-index
/// slice `&content_hash[..8]` at manage.rs:128 panics because byte 8 falls
/// mid-character. Char-safe truncation (`chars().take(8)`) is required.
///
/// The mesh file is hand-written with a hash field whose bytes straddle the
/// 8-byte boundary: `café` is 4 chars / 5 bytes (é = 2 bytes), and `🍕` is
/// 4 bytes. Byte 8 lands inside the first `🍕`, so a byte-index `[..8]`
/// slice panics. With `content_hash` longer than 8 bytes, the `.min()` guard
/// does not help — the slice still lands mid-character.
#[test]
fn mesh_show_non_ascii_hash_does_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Create a mesh file whose content_hash has multi-byte UTF-8 at the
    // 8-byte slice boundary. The hash value `café🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕`
    // places byte 8 inside the first 🍕 (bytes 5..9).
    //
    // Content_hash byte layout:
    //   c     a     f     é         🍕
    //   0     1     2     3-4       5-8   ...
    //
    // content_hash[..8] requires byte 8 to be a char boundary, but it is
    // the last byte of 🍕 (0x95, a UTF-8 continuation byte) → panic.
    std::fs::create_dir_all(root.join(".wiki")).unwrap();
    std::fs::write(
        root.join(".wiki/test-mesh"),
        "path/to/file.rs sha256:café🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕🍕\n\nwhy text\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // The wiki binary will panic here against unfixed code because
    // `content_hash[..8]` lands mid-character. Once fixed (char-safe
    // truncation), exit 0 and the assertion passes.
    let out = wiki(root, &["mesh", "show", "test-mesh"]);
    assert!(
        out.status.success(),
        "wiki mesh show must not panic; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // With char-safe truncation, the first 8 characters are displayed.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("caf"),
        "output should contain the start of the truncated hash:\n{stdout}"
    );
}
