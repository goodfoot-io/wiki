//! Integration test for `wiki scaffold` against a captured baseline
//! (`tests/fixtures/mesh-scaffold/expected.md`).
//!
//! `expected.md` is a Rust-output regression baseline — not a JS-oracle
//! capture. The JS prototype had bugs (raw anchor paths, shell-unsafe why
//! interpolation) that the Rust port deliberately diverges from; once those
//! are fixed in Rust, the baseline is regenerated from the corrected output
//! and locks future changes against accidental regression.
//!
//! The test stages the fixture wiki tree into a temporary git repo (the Rust
//! binary requires a git root for path resolution), runs the compiled binary
//! with cwd set to the temp repo, and asserts byte-equality against
//! `expected.md`.

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
fn mesh_scaffold_byte_equal_with_expected_md() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mesh-scaffold");
    let expected_path = fixture_dir.join("expected.md");
    let expected =
        std::fs::read(&expected_path).expect("tests/fixtures/mesh-scaffold/expected.md must exist");

    // Stage fixture into a fresh tempdir + git repo so the Rust binary can
    // resolve a repo root and discover wiki files.
    let tmp = tempfile::tempdir().unwrap();
    copy_dir_recursive(&fixture_dir.join("wiki"), &tmp.path().join("wiki"));
    copy_dir_recursive(&fixture_dir.join("src"), &tmp.path().join("src"));
    // Mark the wiki/ dir as a wiki root so `WikiConfig::load` finds it.
    std::fs::write(tmp.path().join("wiki/wiki.toml"), "").unwrap();

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
    // Run from inside the wiki dir so `WikiConfig::load` walks up and finds
    // `wiki/wiki.toml`.
    let output = Command::new(bin)
        .args(["scaffold"])
        .current_dir(tmp.path().join("wiki"))
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki scaffold failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    if output.stdout != expected {
        let got = String::from_utf8_lossy(&output.stdout);
        let want = String::from_utf8_lossy(&expected);
        panic!(
            "wiki scaffold output diverged from expected.md\n--- got ---\n{got}\n--- want ---\n{want}"
        );
    }
}

#[test]
fn mesh_scaffold_parse_error_block() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // Category 1: NoFrontmatter — file does not start with `---`.
    std::fs::write(wiki_dir.join("no_frontmatter.md"), "# Just a body.\n").unwrap();

    // Category 2: MissingTitle — frontmatter present but no `title:` key.
    std::fs::write(
        wiki_dir.join("missing_title.md"),
        "---\nsummary: x\n---\n\nbody\n",
    )
    .unwrap();

    // Category 3: EmptyTitle — `title:` present but empty.
    std::fs::write(
        wiki_dir.join("empty_title.md"),
        "---\ntitle:\nsummary: x\n---\n\nbody\n",
    )
    .unwrap();

    // Category 4: Unreadable — non-UTF-8 bytes.
    std::fs::write(wiki_dir.join("unreadable.md"), [0xFF_u8, 0xFE, 0x00]).unwrap();

    // Clean file with a fragment link so there are meshes in the output.
    std::fs::write(
        wiki_dir.join("clean.md"),
        "---\ntitle: Clean Page\nsummary: A clean page.\n---\n\nSee [lib](src/lib.rs#L1-L5) for details.\n",
    )
    .unwrap();

    // Create a src/lib.rs file that the link points to.
    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("lib.rs"), "// lib\n").unwrap();

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
        .args(["scaffold"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki scaffold failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // The output must begin with the parse-error block.
    assert!(
        stdout.starts_with("Unable to generate scaffolding due to parsing errors:\n"),
        "expected parse-error block at start, got:\n{stdout}"
    );

    // All four bad files must be listed with their exact reason strings.
    // File names are sorted: empty_title < missing_title < no_frontmatter < unreadable
    assert!(
        stdout.contains("wiki/empty_title.md (frontmatter present but `title:` is empty)"),
        "EmptyTitle reason missing:\n{stdout}"
    );
    assert!(
        stdout.contains("wiki/missing_title.md (frontmatter present but `title:` is missing)"),
        "MissingTitle reason missing:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "wiki/no_frontmatter.md (no frontmatter block — file does not start with `---`)"
        ),
        "NoFrontmatter reason missing:\n{stdout}"
    );
    assert!(
        stdout.contains("wiki/unreadable.md (file could not be read:"),
        "Unreadable reason missing:\n{stdout}"
    );

    // Clean file is not in the parse-error block.
    let block_end = stdout.find("\n---\n").unwrap_or(stdout.len());
    assert!(
        !stdout[..block_end].contains("clean.md"),
        "clean.md should not appear in the parse-error block:\n{stdout}"
    );

    // The separator `---` must appear after the parse-error block.
    assert!(
        stdout.contains("\n---\n"),
        "expected `---` separator after parse-error block:\n{stdout}"
    );

    // The clean page section must appear after the separator.
    let sep_pos = stdout.find("\n---\n").unwrap();
    let after_sep = &stdout[sep_pos..];
    assert!(
        after_sep.contains("Clean Page"),
        "clean page section expected after separator:\n{stdout}"
    );
}

/// Every discovered wiki file fails to parse → only the parse-error block,
/// no `---` separator, no success line.
#[test]
fn mesh_scaffold_only_parse_errors_emits_block_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // Only files that fail to parse — no clean file with links.
    std::fs::write(wiki_dir.join("no_fm.md"), "# No frontmatter.\n").unwrap();
    std::fs::write(
        wiki_dir.join("missing_title.md"),
        "---\nsummary: x\n---\n\nbody\n",
    )
    .unwrap();

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
        .args(["scaffold"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki scaffold failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.starts_with("Unable to generate scaffolding due to parsing errors:\n"),
        "expected parse-error block at start, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("\n---\n"),
        "separator must be absent when only parse errors:\n{stdout}"
    );
    assert!(
        !stdout.contains("No uncovered fragment links"),
        "success line must be absent when only parse errors:\n{stdout}"
    );
    assert!(
        !stdout.contains("# wiki scaffold"),
        "success header must be absent when only parse errors:\n{stdout}"
    );
}

/// JSON mode against a wiki with an unreadable (non-UTF-8) file must exit
/// non-zero with a diagnostic — preserving baseline hard-error semantics.
#[test]
fn mesh_scaffold_json_unreadable_file_exits_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // A valid file so the wiki is non-empty.
    std::fs::write(
        wiki_dir.join("clean.md"),
        "---\ntitle: Clean\nsummary: s\n---\n\nSee [x](src/x.rs#L1-L2).\n",
    )
    .unwrap();
    // Non-UTF-8 bytes → unreadable.
    std::fs::write(wiki_dir.join("unreadable.md"), [0xFF_u8, 0xFE, 0x00]).unwrap();

    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("x.rs"), "// x\n").unwrap();

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
        .args(["scaffold", "--json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit for unreadable file in JSON mode, got success.\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
