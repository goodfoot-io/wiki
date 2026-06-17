//! Integration tests for anchor-staleness checking in `wiki check`.
//!
//! `wiki check` exits non-zero when a cited anchor's source content changed in
//! place or was deleted; `wiki check --fix` auto-settles whitespace-only drift
//! and moves, and surfaces meaning-change / deleted drift fail-closed with a
//! copy-pasteable remediation command.
//!
//! Meshes are seeded directly into `.wiki/<slug>` using the same rk64 hashing
//! the CLI uses (the algorithm field is `rk64`; `rk64_from_hex` parses the
//! stored hash), so no binary round-trip is needed to mint a fresh anchor.

use std::path::Path;
use std::process::{Command, Output};

use git_mesh_core::mesh_file::{AnchorRecord, MeshFile};
use git_mesh_core::{AnchorExtent, cheap_fingerprint_with_extent, rk64_to_hex};

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

/// Run `wiki check` with the supplied extra args from `cwd`.
fn wiki_check(cwd: &Path, extra: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let mut args = vec!["check"];
    args.extend_from_slice(extra);
    args.push("**/*.md");
    Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("run wiki check")
}

/// Write a wiki page (frontmatter + body) under `wiki/<name>`.
fn write_page(root: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    let content = format!("---\ntitle: {name}\nsummary: A page about {name}.\n---\n\n{body}\n");
    std::fs::write(root.join("wiki").join(name), content).unwrap();
}

/// Seed a `.wiki/<slug>` mesh anchoring `path` over `extent`, hashing the given
/// bytes (which must equal the committed source content) as rk64.
fn seed_mesh(root: &Path, slug: &str, path: &str, extent: AnchorExtent, source_bytes: &[u8]) {
    let hash = rk64_to_hex(cheap_fingerprint_with_extent(source_bytes, &extent));
    let (start, end) = match extent {
        AnchorExtent::WholeFile => (0, 0),
        AnchorExtent::LineRange { start, end } => (start, end),
    };
    let mesh = MeshFile {
        anchors: vec![AnchorRecord {
            path: path.to_string(),
            start_line: start,
            end_line: end,
            algorithm: "rk64".to_string(),
            content_hash: hash,
        }],
        why: String::new(),
    };
    let slug_dir = root.join(".wiki");
    std::fs::create_dir_all(&slug_dir).unwrap();
    std::fs::write(slug_dir.join(slug), mesh.serialize()).unwrap();
}

/// Create a mesh via the real `wiki mesh add` CLI (covers page + target so the
/// existing coverage checks pass and the move-follow can rewrite the page link).
fn mesh_add(cwd: &Path, slug: &str, anchors: &[&str]) {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let mut args = vec!["mesh", "add", slug];
    args.extend_from_slice(anchors);
    args.extend_from_slice(&["--why", "test rationale"]);
    let out = Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("run wiki mesh add");
    assert!(
        out.status.success(),
        "wiki mesh add {anchors:?} failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    tmp
}

// ── Test 1 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_committed_change_exits_nonzero() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 1, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Edit the source at the same line range and commit.
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    99\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "change"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stale committed change must exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.to_lowercase().contains("anchor"),
        "expected an anchor_stale diagnostic; got:\n{combined}"
    );
    assert!(
        combined.contains("wiki mesh add"),
        "meaning-change diagnostic must include `wiki mesh add`; got:\n{combined}"
    );
}

// ── Test 2 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_no_masking_after_reindex() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 1, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n    99\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "change"]);

    let first = wiki_check(root, &[]);
    assert_eq!(first.status.code(), Some(1), "first run must exit 1");

    // Force an index refresh between runs (a search rebuilds the index).
    let bin = env!("CARGO_BIN_EXE_wiki");
    let _ = Command::new(bin)
        .args(["foo"])
        .current_dir(root)
        .output()
        .expect("run wiki search");

    let second = wiki_check(root, &[]);
    assert_eq!(
        second.status.code(),
        Some(1),
        "second run after reindex must still exit 1 (no masking); stderr=\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
}

// ── Test 3 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_fix_whitespace_only_uncommitted() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 1, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Whitespace-only reformat (trailing spaces), NOT committed.
    std::fs::write(root.join("src/lib.rs"), "fn foo() {   \n    42  \n}\n").unwrap();

    let out = wiki_check(root, &["--fix", "--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "whitespace-only drift must auto-settle and exit 0; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|l| l == ".wiki/myslug"),
        "refreshed mesh path must appear in --print-applied stdout; got:\n{stdout}"
    );

    // Hash was refreshed: a bare check now passes.
    let after = wiki_check(root, &[]);
    assert_eq!(after.status.code(), Some(0), "after refresh, bare check passes");
}

// ── Test 4 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_fix_whitespace_only_committed() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 1, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Whitespace-only reformat, then COMMIT it (HEAD no longer holds old bytes).
    std::fs::write(root.join("src/lib.rs"), "fn foo() {   \n    42  \n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "reformat"]);

    let out = wiki_check(root, &["--fix", "--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "committed whitespace drift must auto-settle via history walk and exit 0; \
         stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|l| l == ".wiki/myslug"),
        "refreshed mesh path must appear in --print-applied stdout; got:\n{stdout}"
    );
}

// ── Test 5 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_fix_meaning_change_exits_nonzero() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 1, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Semantic change.
    std::fs::write(root.join("src/lib.rs"), "fn bar() {\n    7\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "rename fn"]);

    let out = wiki_check(root, &["--fix"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "meaning change must exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("wiki mesh add myslug src/lib.rs#L1-L3"),
        "SkippedFix must carry the exact `wiki mesh add <slug> <path>#Lrange`; got:\n{combined}"
    );
}

// ── Test 6 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_fix_deleted_range_exits_nonzero() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n    extra1\n    extra2\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 3, end: 5 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Delete the cited range — file becomes shorter so L3-L5 is out of bounds.
    std::fs::write(root.join("src/lib.rs"), "fn foo() {\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "shrink"]);

    let out = wiki_check(root, &["--fix"]);
    assert_eq!(out.status.code(), Some(1), "deleted range must exit 1");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("wiki mesh remove myslug src/lib.rs#L3-L5"),
        "deleted-anchor reason must carry `wiki mesh remove <slug> <anchor>`; got:\n{combined}"
    );

    // Round-trip: run the printed `wiki mesh remove`, then bare check passes.
    let bin = env!("CARGO_BIN_EXE_wiki");
    let rm = Command::new(bin)
        .args(["mesh", "remove", "myslug", "src/lib.rs#L3-L5"])
        .current_dir(root)
        .output()
        .expect("run wiki mesh remove");
    assert!(
        rm.status.success(),
        "wiki mesh remove must succeed; stderr=\n{}",
        String::from_utf8_lossy(&rm.stderr)
    );

    let after = wiki_check(root, &[]);
    assert_eq!(
        after.status.code(),
        Some(0),
        "after removing the dead anchor, bare check passes; stderr=\n{}",
        String::from_utf8_lossy(&after.stderr)
    );
}

// ── Test 7 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_no_exit_code_suppresses() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 1, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    std::fs::write(root.join("src/lib.rs"), "fn bar() {\n    7\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "change"]);

    let out = wiki_check(root, &["--fix", "--no-exit-code"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--no-exit-code suppresses the staleness exit; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Test 8 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_no_anchors_exits_zero() {
    let tmp = init_repo();
    let root = tmp.path();
    write_page(root, "page.md", "Just prose, no citations.");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a wiki with no mesh citations exits 0; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Test 9 ───────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_skips_md_page_anchors() {
    let tmp = init_repo();
    let root = tmp.path();

    // A page-anchor: WholeFile extent targeting a .md file.
    let spec = "---\ntitle: Spec\nsummary: Some spec.\n---\n\nOriginal prose.\n";
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/spec.md"), spec).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "docs/spec.md", AnchorExtent::WholeFile, spec.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Prose edit to the .md page-anchor target.
    let edited = "---\ntitle: Spec\nsummary: Some spec.\n---\n\nRewritten prose entirely.\n";
    std::fs::write(root.join("docs/spec.md"), edited).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "edit prose"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "page-anchor classification is skipped → exit 0; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Test 10 ──────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_tolerates_conflicted_mesh() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");

    // A .wiki/ file with conflict markers — skipped by read_all_tolerant.
    std::fs::create_dir_all(root.join(".wiki")).unwrap();
    std::fs::write(
        root.join(".wiki/conflicted"),
        "<<<<<<< HEAD\nsrc/lib.rs rk64:aaaaaaaaaaaaaaaa\n=======\nsrc/lib.rs rk64:bbbbbbbbbbbbbbbb\n>>>>>>> other\n\nWhy.\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Bare check reports the conflict but the staleness pass does not panic.
    let out = wiki_check(root, &[]);
    // A conflicted mesh produces a mesh_conflict diagnostic → exit 1, but the
    // run must complete cleanly (not exit 2 / crash).
    assert_ne!(
        out.status.code(),
        Some(2),
        "conflicted mesh must be tolerated, not a runtime error; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Test 11 ──────────────────────────────────────────────────────────────────

#[test]
fn anchor_md_page_anchor_move_follow_preserved() {
    let tmp = init_repo();
    let root = tmp.path();

    // Page-anchor (WholeFile, .md) whose content moves to a new path. The wiki
    // page links to it whole-file so the move-follow can rewrite the link and
    // relocate the stored anchor.
    let doc = "---\ntitle: Doc\nsummary: A doc.\n---\n\nUnique movable body.\n";
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/old.md"), doc).unwrap();
    write_page(root, "page.md", "See [doc](../docs/old.md).");
    mesh_add(root, "myslug", &["wiki/page.md", "docs/old.md"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Move the .md file to a new path (the verbatim content relocates).
    git(root, &["mv", "docs/old.md", "docs/new.md"]);

    let out = wiki_check(root, &["--fix"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "page-anchor move must be auto-followed and exit 0; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The `.md` classification-skip must NOT have turned the moved page-anchor
    // into a false Changed/Deleted drift: no anchor-staleness failure is raised
    // (the moved page-anchor is handled as a move, silently). The link itself is
    // auto-followed to the new path by the rename machinery.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("Anchor Stale") && !combined.contains("anchor_stale"),
        "moved page-anchor must not be misclassified as anchor staleness; got:\n{combined}"
    );
}

// ── Test 12 ──────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_source_head_dirty_worktree() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 1, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Commit a meaning change (stale vs stored hash in HEAD).
    std::fs::write(root.join("src/lib.rs"), "fn bar() {\n    7\n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "change"]);

    // Add a further worktree-only edit to the same file.
    std::fs::write(root.join("src/lib.rs"), "fn baz() {\n    8\n}\n").unwrap();

    let head = wiki_check(root, &["--source=head"]);
    assert_eq!(
        head.status.code(),
        Some(1),
        "HEAD content is stale vs stored hash → exit 1; stderr=\n{}",
        String::from_utf8_lossy(&head.stderr)
    );

    let work = wiki_check(root, &["--source=worktree"]);
    assert_eq!(
        work.status.code(),
        Some(1),
        "worktree content is stale vs stored hash → exit 1; stderr=\n{}",
        String::from_utf8_lossy(&work.stderr)
    );
}

// ── Test 13 ──────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_deleted_no_trailing_newline() {
    let tmp = init_repo();
    let root = tmp.path();

    // Cited last line has NO trailing newline.
    let src = "line one\nline two\nline three";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "src/lib.rs", AnchorExtent::LineRange { start: 3, end: 3 }, src.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Edit the last line in place (still 3 lines, still no trailing newline).
    std::fs::write(root.join("src/lib.rs"), "line one\nline two\nline THREE").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "edit last line"]);

    let out = wiki_check(root, &["--fix"]);
    assert_eq!(out.status.code(), Some(1), "edited last line is stale → exit 1");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // It must be classified as Changed (meaning), NOT Deleted/out-of-bounds.
    assert!(
        combined.contains("wiki mesh add myslug src/lib.rs#L3-L3"),
        "last-line edit without trailing newline must classify as Changed, not Deleted; \
         got:\n{combined}"
    );
    assert!(
        !combined.contains("wiki mesh remove myslug src/lib.rs#L3-L3"),
        "must NOT be classified as Deleted; got:\n{combined}"
    );
}

// ── Test 14 ──────────────────────────────────────────────────────────────────

#[test]
fn anchor_committed_same_file_move_auto_followed() {
    let tmp = init_repo();
    let root = tmp.path();

    // A unique function body at lines 1-3; padding follows.
    let body = "UNIQUE_MARKER_A\nUNIQUE_MARKER_B\nUNIQUE_MARKER_C\n";
    let src = format!("{body}pad1\npad2\npad3\npad4\npad5\npad6\npad7\n");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), &src).unwrap();
    // The wiki page links to the block so the move-follow can rewrite the link
    // and relocate the stored anchor.
    write_page(root, "page.md", "See [code](../src/lib.rs#L1-L3).");
    mesh_add(root, "myslug", &["wiki/page.md", "src/lib.rs#L1-L3"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Move the unique block down to lines 8-10; commit (clean worktree).
    let moved = format!("pad1\npad2\npad3\npad4\npad5\npad6\npad7\n{body}");
    std::fs::write(root.join("src/lib.rs"), &moved).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "move block"]);

    let out = wiki_check(root, &["--fix"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "committed same-file move must auto-follow via own-file scan and exit 0; \
         stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mesh = std::fs::read_to_string(root.join(".wiki/myslug")).unwrap();
    assert!(
        mesh.contains("src/lib.rs#L8-L10"),
        "own-file move-follow must relocate the anchor to L8-L10; got:\n{mesh}"
    );
}

// ── Test 15 ──────────────────────────────────────────────────────────────────

#[test]
fn anchor_stale_md_source_line_range_detected() {
    let tmp = init_repo();
    let root = tmp.path();

    // A line-range citation of a .md file (NOT a WholeFile page-anchor).
    let spec = "# Spec\n\nLine three of spec.\nLine four of spec.\n";
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/spec.md"), spec).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(root, "myslug", "docs/spec.md", AnchorExtent::LineRange { start: 3, end: 4 }, spec.as_bytes());
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Edit lines 3-4 in place.
    let edited = "# Spec\n\nCompletely different line three.\nAnd a new line four.\n";
    std::fs::write(root.join("docs/spec.md"), edited).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "edit spec lines"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "line-range .md citation drift must be detected (page-anchor skip does not apply); \
         stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.to_lowercase().contains("anchor"),
        "expected an anchor_stale diagnostic for the line-range .md citation; got:\n{combined}"
    );
}
