//! Integration test for `wiki mesh scaffold` parity with the captured JS
//! prototype output (`tests/fixtures/mesh-scaffold/expected.sh`).
//!
//! The test stages the fixture wiki tree into a temporary git repo (the Rust
//! binary requires a git root for path resolution), runs the compiled binary
//! with cwd set to the temp repo, and asserts byte-equality against
//! `expected.sh`.

use std::path::Path;
use std::process::Command;

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path);
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &dst_path).unwrap();
        }
    }
}

fn git(workdir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn mesh_scaffold_byte_equal_with_expected_sh() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mesh-scaffold");
    let expected_path = fixture_dir.join("expected.sh");
    let expected =
        std::fs::read(&expected_path).expect("tests/fixtures/mesh-scaffold/expected.sh must exist");

    // Stage fixture into a fresh tempdir + git repo so the Rust binary can
    // resolve a repo root and discover wiki files.
    let tmp = tempfile::tempdir().unwrap();
    copy_dir_recursive(&fixture_dir.join("wiki"), &tmp.path().join("wiki"));
    copy_dir_recursive(&fixture_dir.join("src"), &tmp.path().join("src"));

    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(
        tmp.path(),
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
    );
    git(
        tmp.path(),
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["mesh", "scaffold"])
        .current_dir(tmp.path())
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki mesh scaffold failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    if output.stdout != expected {
        let got = String::from_utf8_lossy(&output.stdout);
        let want = String::from_utf8_lossy(&expected);
        panic!(
            "wiki mesh scaffold output diverged from expected.sh\n\
             --- got ---\n{got}\n--- want ---\n{want}"
        );
    }
}
