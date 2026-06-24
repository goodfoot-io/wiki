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
    let slug_path = root.join(".wiki").join(slug);
    std::fs::create_dir_all(slug_path.parent().unwrap()).unwrap();
    std::fs::write(slug_path, mesh.serialize()).unwrap();
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

// ── Test 16 (F-FM2) ────────────────────────────────────────────────────────────

/// A wiki page A line-range-cites a DIFFERENT wiki page B. A meaning change to
/// B's cited lines must be detected — the narrowed self-section exemption only
/// covers a page citing its OWN section, not a cross-page citation into another
/// wiki page.
#[test]
fn anchor_cross_page_wiki_line_range_citation_detected() {
    let tmp = init_repo();
    let root = tmp.path();

    // Page B is a wiki page (title + summary) whose body cites a rate limit.
    let page_b = "---\ntitle: Limits\nsummary: Rate limits doc.\n---\n\n\
                  The cap is 100 requests per second.\nMore prose.\n";
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(root.join("wiki/limits.md"), page_b).unwrap();
    // Page A is the citing page; its mesh slug lives under wiki/, but the cited
    // anchor targets page B (a different page) on a line range.
    write_page(root, "page.md", "See [limits](./limits.md#L6).");
    // Seed a mesh slug that is page A's (wiki/...), anchoring page B's line 6.
    seed_mesh(
        root,
        "page-limits-ref",
        "wiki/limits.md",
        AnchorExtent::LineRange { start: 6, end: 6 },
        page_b.as_bytes(),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Change the MEANING of the cited line in page B.
    let edited = "---\ntitle: Limits\nsummary: Rate limits doc.\n---\n\n\
                  The cap is 5000 requests per second.\nMore prose.\n";
    std::fs::write(root.join("wiki/limits.md"), edited).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "raise cap"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "cross-page wiki line-range citation drift must be detected; stdout=\n{}\nstderr=\n{}",
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
        "expected an anchor_stale diagnostic for the cross-page citation; got:\n{combined}"
    );
}

// ── Test 17 (F-FM2) ────────────────────────────────────────────────────────────

/// A wiki page's scaffold-managed SELF-section anchor (a line-range `.md` anchor
/// whose page is the mesh's own owning page) stays exempt: a prose edit to the
/// page's own cited section does not raise an anchor-staleness failure.
#[test]
fn anchor_self_section_wiki_anchor_exempt() {
    let tmp = init_repo();
    let root = tmp.path();

    // A wiki page under wiki/ whose own section is anchored by ITS OWN mesh
    // (slug `wiki/<noun>` — page directory `wiki` is the slug prefix).
    let page = "---\ntitle: Self\nsummary: A self page.\n---\n\n\
                ## Section\n\nOriginal section prose.\n";
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(root.join("wiki/self.md"), page).unwrap();
    seed_mesh(
        root,
        "wiki/section",
        "wiki/self.md",
        AnchorExtent::LineRange { start: 6, end: 8 },
        page.as_bytes(),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Edit the page's own section prose and commit.
    let edited = "---\ntitle: Self\nsummary: A self page.\n---\n\n\
                  ## Section\n\nCompletely rewritten section prose here.\n";
    std::fs::write(root.join("wiki/self.md"), edited).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "edit own section"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "scaffold self-section anchor must stay exempt → exit 0; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Test 18 (F-INDENT) ─────────────────────────────────────────────────────────

/// An indentation-only reformat (2→4 spaces) of a cited range carries no token
/// change, so `wiki check --fix` re-anchors it as whitespace-only and exits 0 —
/// it is NOT a SkippedFix.
#[test]
fn anchor_fix_indentation_only_is_whitespace_equivalent() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n  let x = 1;\n  bar(x);\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(
        root,
        "myslug",
        "src/lib.rs",
        AnchorExtent::LineRange { start: 1, end: 4 },
        src.as_bytes(),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Reindent the cited range from 2 to 4 spaces; no token changes. Committed.
    let reindented = "fn foo() {\n    let x = 1;\n    bar(x);\n}\n";
    std::fs::write(root.join("src/lib.rs"), reindented).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "rustfmt reindent"]);

    let out = wiki_check(root, &["--fix", "--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "indentation-only reformat must auto-settle as whitespace-only → exit 0; \
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
        !combined.contains("wiki mesh add"),
        "indentation-only reformat must NOT be a SkippedFix; got:\n{combined}"
    );
}

// ── Test 19 (F-FM1) ────────────────────────────────────────────────────────────

// ── Test 18b (F-INDENT, indentation-significant) ────────────────────────────────

/// Seed a multi-anchor mesh: a leading owning-page self-section anchor followed
/// by the listed *target* anchors. Models the scaffold's emitted shape (owning
/// page section first, citations after).
fn seed_mesh_multi(
    root: &Path,
    slug: &str,
    leading: (&str, AnchorExtent, &[u8]),
    targets: &[(&str, AnchorExtent, &[u8])],
) {
    let mut anchors: Vec<AnchorRecord> = Vec::new();
    for (path, extent, bytes) in std::iter::once(leading).chain(targets.iter().copied()) {
        let hash = rk64_to_hex(cheap_fingerprint_with_extent(bytes, &extent));
        let (start, end) = match extent {
            AnchorExtent::WholeFile => (0, 0),
            AnchorExtent::LineRange { start, end } => (start, end),
        };
        anchors.push(AnchorRecord {
            path: path.to_string(),
            start_line: start,
            end_line: end,
            algorithm: "rk64".to_string(),
            content_hash: hash,
        });
    }
    let mesh = MeshFile {
        anchors,
        why: String::new(),
    };
    let slug_path = root.join(".wiki").join(slug);
    std::fs::create_dir_all(slug_path.parent().unwrap()).unwrap();
    std::fs::write(slug_path, mesh.serialize()).unwrap();
}

/// A `.py` dedent that moves a statement OUT of an `if` block changes meaning
/// purely through leading indentation. Python is indentation-significant, so the
/// edit must NOT be graded whitespace-only: `wiki check --fix` fails closed
/// (exit 1, SkippedFix) and bare `wiki check` exits 1.
#[test]
fn anchor_fix_python_dedent_is_not_whitespace_only() {
    let tmp = init_repo();
    let root = tmp.path();

    // Cited lines 1-3: a guarded call nested inside the `if`.
    let src = "if cond:\n    do_thing()\n    cleanup()\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/app.py"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(
        root,
        "myslug",
        "src/app.py",
        AnchorExtent::LineRange { start: 1, end: 3 },
        src.as_bytes(),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Dedent `cleanup()` out of the `if` — now it always runs. Indentation-only
    // diff, but the meaning changed. Committed.
    let dedented = "if cond:\n    do_thing()\ncleanup()\n";
    std::fs::write(root.join("src/app.py"), dedented).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "dedent cleanup"]);

    let out = wiki_check(root, &["--fix"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "python dedent (meaning change) must NOT auto-settle → exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("wiki mesh add myslug src/app.py#L1-L3"),
        "python dedent must surface as a SkippedFix with a re-anchor command; got:\n{combined}"
    );

    let bare = wiki_check(root, &[]);
    assert_eq!(
        bare.status.code(),
        Some(1),
        "bare check on the python dedent must exit 1; stderr=\n{}",
        String::from_utf8_lossy(&bare.stderr)
    );
}

/// A `.yaml` indentation change re-nests a key under a different parent —
/// meaning-changing in an indentation-significant format. It must NOT be graded
/// whitespace-only.
#[test]
fn anchor_fix_yaml_indentation_is_not_whitespace_only() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "root:\n  child: 1\n  other: 2\n";
    std::fs::create_dir_all(root.join("cfg")).unwrap();
    std::fs::write(root.join("cfg/app.yaml"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(
        root,
        "myslug",
        "cfg/app.yaml",
        AnchorExtent::LineRange { start: 1, end: 3 },
        src.as_bytes(),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Dedent `other` to the top level — re-parents it. Indentation-only diff.
    let reindented = "root:\n  child: 1\nother: 2\n";
    std::fs::write(root.join("cfg/app.yaml"), reindented).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "reindent yaml"]);

    let out = wiki_check(root, &["--fix"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "yaml indentation change must NOT auto-settle → exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Test 16b (F-FM2, same-directory sibling) ─────────────────────────────────────

/// Page A line-range-cites a SAME-DIRECTORY sibling wiki page B. The scaffolded
/// mesh leads with A's own section anchor, then carries B's cited range as a
/// target. A meaning change to B's cited lines MUST be detected — the
/// self-section exemption fires only for the mesh's leading owning-page anchor,
/// never a same-directory cross-page citation.
#[test]
fn anchor_same_dir_sibling_cross_page_citation_detected() {
    let tmp = init_repo();
    let root = tmp.path();

    std::fs::create_dir_all(root.join("wiki/meta")).unwrap();
    // Owning page A.
    let page_a = "---\ntitle: Ref\nsummary: A reference page.\n---\n\n\
                  ## Refs\n\nSee the concept cap.\n";
    std::fs::write(root.join("wiki/meta/ref.md"), page_a).unwrap();
    // Sibling page B (same directory).
    let page_b = "---\ntitle: Concept\nsummary: A concept page.\n---\n\n\
                  The cap is 100 per second.\nMore.\n";
    std::fs::write(root.join("wiki/meta/concept.md"), page_b).unwrap();

    // Mesh is A's (slug under wiki/meta): leading anchor = A's own section,
    // target = B's line 6 (a same-directory cross-page citation).
    seed_mesh_multi(
        root,
        "wiki/meta/refs",
        (
            "wiki/meta/ref.md",
            AnchorExtent::LineRange { start: 6, end: 8 },
            page_a.as_bytes(),
        ),
        &[(
            "wiki/meta/concept.md",
            AnchorExtent::LineRange { start: 6, end: 6 },
            page_b.as_bytes(),
        )],
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Meaning change to B's cited line.
    let edited_b = "---\ntitle: Concept\nsummary: A concept page.\n---\n\n\
                    The cap is 5000 per second.\nMore.\n";
    std::fs::write(root.join("wiki/meta/concept.md"), edited_b).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "raise cap"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "same-directory sibling cross-page citation drift must be detected; \
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
        "expected an anchor_stale diagnostic for the sibling citation; got:\n{combined}"
    );
}

// ── Test 16c (F-FM2, ancestor-directory) ─────────────────────────────────────────

/// Page A (under wiki/meta) line-range-cites a page B in an ANCESTOR directory
/// (wiki/). A meaning change to B's cited lines MUST be detected: B is a target,
/// not the mesh's leading owning-page anchor.
#[test]
fn anchor_ancestor_dir_cross_page_citation_detected() {
    let tmp = init_repo();
    let root = tmp.path();

    std::fs::create_dir_all(root.join("wiki/meta")).unwrap();
    let page_a = "---\ntitle: Ref\nsummary: A reference page.\n---\n\n\
                  ## Refs\n\nSee the parent cap.\n";
    std::fs::write(root.join("wiki/meta/ref.md"), page_a).unwrap();
    // Ancestor-directory page B (directly under wiki/).
    let page_b = "---\ntitle: Top\nsummary: A top page.\n---\n\n\
                  The cap is 100 per second.\nMore.\n";
    std::fs::write(root.join("wiki/top.md"), page_b).unwrap();

    seed_mesh_multi(
        root,
        "wiki/meta/refs",
        (
            "wiki/meta/ref.md",
            AnchorExtent::LineRange { start: 6, end: 8 },
            page_a.as_bytes(),
        ),
        &[(
            "wiki/top.md",
            AnchorExtent::LineRange { start: 6, end: 6 },
            page_b.as_bytes(),
        )],
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    let edited_b = "---\ntitle: Top\nsummary: A top page.\n---\n\n\
                    The cap is 5000 per second.\nMore.\n";
    std::fs::write(root.join("wiki/top.md"), edited_b).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "raise cap"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "ancestor-directory cross-page citation drift must be detected; \
         stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A committed whitespace-only edit in a FULL-history repo is classified
/// whitespace-only and auto-settled by `--fix` (the recoverable path).
#[test]
fn anchor_fix_whitespace_committed_full_history_settles() {
    let tmp = init_repo();
    let root = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    write_page(root, "page.md", "Documentation prose.");
    seed_mesh(
        root,
        "myslug",
        "src/lib.rs",
        AnchorExtent::LineRange { start: 1, end: 3 },
        src.as_bytes(),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Trailing-whitespace tweak, committed. Pre-edit bytes recoverable from HEAD~.
    std::fs::write(root.join("src/lib.rs"), "fn foo() {   \n    42  \n}\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "ws tweak"]);

    let out = wiki_check(root, &["--fix", "--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "committed whitespace drift in full history must auto-settle → exit 0; \
         stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A committed drift whose pre-edit content is UNRECOVERABLE (shallow clone via
/// `git clone --depth 1`) is fail-closed (exit 1) with a DISTINCT "could not
/// recover pre-edit content" reason — not a false whitespace-only auto-settle
/// and not a misleading "meaning-changing diff" claim.
#[test]
fn anchor_fix_unrecoverable_history_fails_closed_distinctly() {
    let tmp = init_repo();
    let origin = tmp.path();

    let src = "fn foo() {\n    42\n}\n";
    std::fs::create_dir_all(origin.join("src")).unwrap();
    std::fs::write(origin.join("src/lib.rs"), src).unwrap();
    write_page(origin, "page.md", "Documentation prose.");
    seed_mesh(
        origin,
        "myslug",
        "src/lib.rs",
        AnchorExtent::LineRange { start: 1, end: 3 },
        src.as_bytes(),
    );
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "seed"]);

    // A committed whitespace-only tweak — recoverable in a full clone, but the
    // shallow clone below will not carry the pre-edit blob.
    std::fs::write(origin.join("src/lib.rs"), "fn foo() {   \n    42  \n}\n").unwrap();
    git(origin, &["add", "-A"]);
    git(origin, &["commit", "-q", "-m", "ws tweak"]);

    // Shallow clone (depth 1) — the GitHub Actions actions/checkout default.
    let clone_dir = tempfile::tempdir().unwrap();
    let clone_path = clone_dir.path().join("shallow");
    let origin_url = format!("file://{}", origin.display());
    let st = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "-q",
            &origin_url,
            clone_path.to_str().unwrap(),
        ])
        .status()
        .expect("git clone");
    assert!(st.success(), "shallow clone must succeed");

    let out = wiki_check(&clone_path, &["--fix", "--print-applied"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "unrecoverable-history drift must fail closed → exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("could not be recovered"),
        "must surface the distinct unrecoverable-history reason; got:\n{combined}"
    );
    assert!(
        !combined.contains("meaning-changing"),
        "must NOT misclassify unrecoverable drift as a meaning-changing diff; got:\n{combined}"
    );
}

// ── Test 23 (F-SELF-INFLICTED) ───────────────────────────────────────────────

/// A page that links into code AND is mesh-anchored over the line range holding
/// that link. When the code drifts, `--fix` rewrites the link *inside* the
/// anchored region — which would stale that region's anchor. The same pass must
/// re-anchor the region (its only change is the self-inflicted line-ref rewrite),
/// so the immediately-following bare `wiki check` PASSES. Without the fix the
/// `--fix`/`check` pair is non-convergent and blocks every commit through the
/// pre-commit hook.
#[test]
fn anchor_fix_self_inflicted_region_reanchor_converges() {
    let tmp = init_repo();
    let root = tmp.path();

    // code.txt's target lives at L5; the page links it and is anchored over the
    // region (L8-L11) that contains the link (page line 9).
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/code.txt"),
        "line1\nline2\nline3\nline4\nTARGET_CONTENT_X\nline6\nline7\nline8\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(
        root.join("wiki/notes.md"),
        "---\ntitle: Notes\nsummary: Links into code and is mesh-anchored over the holding region.\n---\n\n\
         # Notes\n\n\
         Intro prose on the first content line of the anchored region.\n\
         The target lives at [target](../src/code.txt#L5-L5) — inside the meshed region.\n\
         More prose to round out the anchored region.\n\
         Closing prose line of the region.\n",
    )
    .unwrap();
    // One mesh anchors BOTH the code target and the page region holding the link.
    mesh_add(root, "demo/region", &["src/code.txt#L5-L5", "wiki/notes.md#L8-L11"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Drift the code so the linked target moves L5 → L7; commit.
    std::fs::write(
        root.join("src/code.txt"),
        "NEW_A\nNEW_B\nline1\nline2\nline3\nline4\nTARGET_CONTENT_X\nline6\nline7\nline8\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "drift"]);

    let fixed = wiki_check(root, &["--fix", "--source=worktree"]);
    assert_eq!(
        fixed.status.code(),
        Some(0),
        "--fix must rewrite the link AND re-anchor the region → exit 0; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&fixed.stdout),
        String::from_utf8_lossy(&fixed.stderr)
    );

    // The crux: the tree --fix produced must PASS the very next bare check.
    let gate = wiki_check(root, &[]);
    assert_eq!(
        gate.status.code(),
        Some(0),
        "the gate check after --fix must pass (convergent); stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&gate.stdout),
        String::from_utf8_lossy(&gate.stderr)
    );

    // Both the code anchor (relocated to L7) and the page region anchor remain.
    let mesh = std::fs::read_to_string(root.join(".wiki/demo/region")).unwrap();
    assert!(
        mesh.contains("code.txt#L7-L7") && mesh.contains("notes.md#L8-L11"),
        "code anchor must follow to L7 and the page region must stay anchored:\n{mesh}"
    );
}

// ── Test 24 (F-SELF-INFLICTED, fail-closed guard) ─────────────────────────────

/// The self-inflicted re-anchor must NOT mask a GENUINE meaning change to the
/// anchored region. If the page region carries a real prose edit (stale at plan
/// time) in addition to the self-inflicted link rewrite, `--fix` stays
/// fail-closed: exit 1 with the re-anchor command, and a bare check still fails.
#[test]
fn anchor_fix_self_inflicted_does_not_mask_genuine_region_drift() {
    let tmp = init_repo();
    let root = tmp.path();

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/code.txt"),
        "line1\nline2\nline3\nline4\nTARGET_CONTENT_X\nline6\nline7\nline8\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(
        root.join("wiki/notes.md"),
        "---\ntitle: Notes\nsummary: Links into code and is mesh-anchored over the holding region.\n---\n\n\
         # Notes\n\n\
         Intro prose on the first content line of the anchored region.\n\
         The target lives at [target](../src/code.txt#L5-L5) — inside the meshed region.\n\
         More prose to round out the anchored region.\n\
         Closing prose line of the region.\n",
    )
    .unwrap();
    mesh_add(root, "demo/region", &["src/code.txt#L5-L5", "wiki/notes.md#L8-L11"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "seed"]);

    // Drift the code (forces the self-inflicted link rewrite); commit.
    std::fs::write(
        root.join("src/code.txt"),
        "NEW_A\nNEW_B\nline1\nline2\nline3\nline4\nTARGET_CONTENT_X\nline6\nline7\nline8\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "drift"]);

    // GENUINE meaning change to a NON-link line inside the region (worktree only).
    std::fs::write(
        root.join("wiki/notes.md"),
        "---\ntitle: Notes\nsummary: Links into code and is mesh-anchored over the holding region.\n---\n\n\
         # Notes\n\n\
         Intro prose on the first content line of the anchored region.\n\
         The target lives at [target](../src/code.txt#L5-L5) — inside the meshed region.\n\
         GENUINELY DIFFERENT PROSE that changes the region's meaning.\n\
         Closing prose line of the region.\n",
    )
    .unwrap();

    let fixed = wiki_check(root, &["--fix", "--source=worktree"]);
    assert_eq!(
        fixed.status.code(),
        Some(1),
        "genuine region drift must stay fail-closed → exit 1; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&fixed.stdout),
        String::from_utf8_lossy(&fixed.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&fixed.stdout),
        String::from_utf8_lossy(&fixed.stderr)
    );
    assert!(
        combined.contains("wiki mesh add demo/region wiki/notes.md#L8-L11"),
        "must surface the region re-anchor command (not silently refresh); got:\n{combined}"
    );

    let gate = wiki_check(root, &[]);
    assert_eq!(
        gate.status.code(),
        Some(1),
        "bare check must still fail on the un-acknowledged genuine drift; stderr=\n{}",
        String::from_utf8_lossy(&gate.stderr)
    );
}
