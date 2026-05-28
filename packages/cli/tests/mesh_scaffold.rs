//! Integration tests for the mesh-coverage engine, driven through
//! `wiki check --fix --print-applied`.
//!
//! `wiki scaffold` was folded into `wiki check --fix` (the "Fix #4" pass): a
//! single command rewrites drifted links/anchors AND creates the `git mesh`
//! coverage every fragment link requires, best-effort. The *set of meshes
//! produced for any corpus is unchanged* from the former `scaffold` command,
//! so assertions about emitted mesh paths and contents port over directly.
//!
//! Output contract under `--print-applied`: stdout is EXACTLY the repo-relative
//! `.mesh/<slug>` paths created/renamed this run (one per line); the fix/skip
//! summary, advisories, and post-fix diagnostics go to stderr. So a hook can
//! `xargs git add` stdout directly.

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

/// `git mesh add` rejects a slug whose ancestor mesh was renamed away within
/// the SAME uncommitted working tree: its prefix-collision guard reads the
/// committed HEAD tree, not the index or worktree (verified empirically
/// against git-mesh 1.0.81). The blocker-rename apply path therefore cannot
/// free the slug for an in-run `git mesh add` until the rename is committed —
/// which the engine deliberately never does. Apply-mode integration tests for
/// the rename remedy skip on such a git-mesh, exactly like the `git-mesh not
/// installed` skip; the rename derivation, fail-open, and dry-run paths remain
/// fully covered by unit + dry-run tests.
fn git_mesh_add_sees_uncommitted_rename() -> bool {
    let tmp = match tempfile::tempdir() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let root = tmp.path();
    if std::fs::create_dir_all(root.join("src")).is_err() {
        return false;
    }
    let _ = std::fs::write(root.join("src/x"), "a\nb\nc\n");
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !run(&["init", "-q", "-b", "main"]) {
        return false;
    }
    if !run(&["mesh", "add", "wiki/b", "src/x#L1-L1"]) {
        return false;
    }
    let _ = run(&["-c", "user.email=t", "-c", "user.name=t", "add", "-A"]);
    let _ = run(&[
        "-c",
        "user.email=t",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "-m",
        "i",
    ]);
    let _ = run(&["mesh", "move", "wiki/b", "wiki/tmp"]);
    let _ = run(&["mesh", "move", "wiki/tmp", "wiki/b/index"]);
    run(&["mesh", "add", "wiki/b/leaf", "src/x#L1-L1"])
}

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

fn git_mesh_available() -> bool {
    Command::new("git-mesh").arg("--version").output().is_ok()
}

/// Run `wiki check --fix --print-applied` from `cwd` (best-effort: pass
/// `--no-exit-code` so the run never aborts). Returns the process output.
fn check_fix_print_applied(cwd: &Path, no_exit_code: bool) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let mut args = vec!["check", "--fix", "--print-applied"];
    if no_exit_code {
        args.push("--no-exit-code");
    }
    args.push("**/*.md");
    Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("run wiki check --fix --print-applied")
}

/// Collect every regular file under `dir`, recursively, as owned paths.
fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn walkdir_mesh(mesh_dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut results: Vec<(std::path::PathBuf, String)> = walkdir(mesh_dir)
        .into_iter()
        .map(|p| {
            let content = std::fs::read_to_string(&p).unwrap_or_default();
            (p, content)
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

/// The fixture corpus (which deliberately includes degenerate-excerpt and
/// over-range edge cases) is run through `wiki check --fix --print-applied`.
/// The engine applies a non-empty set of meshes best-effort; every applied
/// path is a real `.mesh/` file that anchors a real fragment link, and stdout
/// stays a clean stage list while advisories for the dropped edge cases go to
/// stderr.
#[test]
fn fixture_corpus_applies_meshes_best_effort() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mesh-scaffold");

    let tmp = tempfile::tempdir().unwrap();
    copy_dir_recursive(&fixture_dir.join("wiki"), &tmp.path().join("wiki"));
    copy_dir_recursive(&fixture_dir.join("src"), &tmp.path().join("src"));

    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-q", "-m", "init"]);

    // Best-effort: --no-exit-code so the run never aborts on the fixture's
    // deliberately-dropped edge cases.
    let output = check_fix_print_applied(tmp.path(), true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "--no-exit-code must exit 0; stdout=\n{stdout}\nstderr=\n{stderr}"
    );

    // stdout is a clean stage list: every non-empty line is a `.mesh/` path on
    // disk that anchors a real link (no advisory/command text leaks).
    let applied: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !applied.is_empty(),
        "expected applied mesh paths; stdout=\n{stdout}"
    );
    for line in &applied {
        assert!(
            line.starts_with(".mesh/"),
            "stdout line not a .mesh/ path: {line:?}"
        );
        let body = std::fs::read_to_string(tmp.path().join(line))
            .unwrap_or_else(|_| panic!("applied path missing on disk: {line}"));
        assert!(
            body.lines().any(|l| l.contains('#')
                || l.ends_with(".md")
                || l.contains(".rs")
                || l.contains(".ts")),
            "applied mesh should anchor at least one path: {line}\n{body}"
        );
    }

    // After applying, the previously-uncovered links that ARE satisfiable are
    // now covered: re-running --fix applies no NEW meshes (idempotent), proving
    // the first pass covered everything coverable.
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-q", "-m", "applied"]);
    let second = check_fix_print_applied(tmp.path(), true);
    assert!(
        second.status.success(),
        "second --no-exit-code run must exit 0"
    );
    let second_applied: Vec<&str> = String::from_utf8_lossy(&second.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|_| "x")
        .collect();
    assert!(
        second_applied.is_empty(),
        "a second fix pass must apply no new meshes (idempotent); stdout=\n{}",
        String::from_utf8_lossy(&second.stdout)
    );
}

/// `--print-applied` emits exactly bare `.mesh/<slug>` paths on stdout; an
/// advisory for a link to a missing path is routed to stderr.
#[test]
fn print_applied_emits_bare_paths_and_advisory_on_stderr() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// lib\n// line2\n// line3\n").unwrap();
    // One valid link (becomes a mesh) plus one link to a missing path (a
    // dropped-mesh advisory) so we can assert advisory routing.
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\n\
         See [lib](../src/lib.rs#L1-L3) for details.\n\n\
         ## Other\n\nSee [gone](../src/gone.rs#L1-L2) here.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // A missing-path link is a residual (uncovered) link, so without
    // --no-exit-code this exits non-zero. We assert routing, not exit code.
    let output = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "--no-exit-code must exit 0; stderr=\n{stderr}"
    );

    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "expected applied mesh paths; stdout=\n{stdout}\nstderr=\n{stderr}"
    );
    for line in &lines {
        assert!(
            line.starts_with(".mesh/"),
            "stdout line not a .mesh/ path: {line:?}"
        );
        assert!(
            root.join(line).is_file(),
            "applied path missing on disk: {line}"
        );
    }

    // Advisory/command text must not leak to stdout (it is an exact stage list).
    assert!(
        !stdout.contains("src/gone.rs") && !stdout.contains("Skipped"),
        "advisory text leaked into stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("src/gone.rs"),
        "expected dropped-path advisory on stderr; stderr=\n{stderr}"
    );
}

/// With no fragment links, `--print-applied` stdout is empty (every non-empty
/// line must be a `.mesh/` path), and no markdown/advisory leaks onto stdout.
#[test]
fn print_applied_empty_when_no_fragment_links() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(
        root.join("wiki/index.md"),
        "---\ntitle: index\nsummary: index page.\n---\n\n# index\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let output = check_fix_print_applied(&root.join("wiki"), false);
    assert!(output.status.success(), "must exit 0 with nothing to do");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|l| !l.is_empty()) {
        assert!(
            line.starts_with(".mesh/"),
            "non-path line leaked to stdout: {line:?}"
        );
    }
    assert!(
        !stdout.contains("# wiki") && !stdout.contains("No internal fragment links"),
        "human-readable text leaked to stdout:\n{stdout}"
    );
}

/// Acceptance: with one injected mesh-creation failure, the run still creates
/// every OTHER mesh, logs the one failure, and `--print-applied` lists exactly
/// the successes. `--no-exit-code` → exit 0 on the residual; plain → non-zero,
/// both after all work.
#[test]
fn best_effort_creates_others_logs_one_failure() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    // First page: a valid 3-line source → mesh will be created.
    std::fs::write(root.join("src/good.rs"), "// line1\n// line2\n// line3\n").unwrap();
    // Second page anchors a *binary* file: path exists and the static
    // line-range check cannot read it as UTF-8 (so the pre-apply drop does not
    // fire), but `git mesh add` itself rejects a line anchor on a binary path
    // mid-run — an injected per-draft failure.
    std::fs::write(root.join("src/bin.dat"), [0x00u8, 0x01, 0xff, 0xfe]).unwrap();
    std::fs::write(
        root.join("wiki/page_a.md"),
        "---\ntitle: Page A\nsummary: A.\n---\n\nSee [good](../src/good.rs#L1-L3) for details.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("wiki/page_b.md"),
        "---\ntitle: Page B\nsummary: B.\n---\n\nSee [bad](../src/bin.dat#L1-L1) for details.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // --no-exit-code → exit 0 despite the residual uncovered link.
    let lenient = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&lenient.stdout);
    let stderr = String::from_utf8_lossy(&lenient.stderr);
    assert!(
        lenient.status.success(),
        "--no-exit-code must exit 0 on a residual; stderr=\n{stderr}"
    );

    // The good mesh was still created and listed; the bad one is NOT listed.
    let applied: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        applied.iter().any(|l| {
            let body = std::fs::read_to_string(root.join(l)).unwrap_or_default();
            body.contains("src/good.rs#L1-L3")
        }),
        "the good mesh must be applied and listed; stdout=\n{stdout}\nstderr=\n{stderr}"
    );
    for line in &applied {
        let body = std::fs::read_to_string(root.join(line)).unwrap_or_default();
        assert!(
            !body.contains("src/bin.dat"),
            "the failed mesh must not appear in the applied list: {line}"
        );
    }
    // The failure is logged on stderr, naming git-mesh's involvement.
    assert!(
        stderr.contains("could not create mesh") || stderr.contains("git mesh"),
        "expected a logged mesh-creation failure on stderr; stderr=\n{stderr}"
    );

    // Re-stage so the second run starts from the same baseline plus the meshes
    // created by the lenient run (idempotent). Plain --fix (no --no-exit-code)
    // exits non-zero because the residual uncovered link remains.
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "applied"]);
    let strict = check_fix_print_applied(&root.join("wiki"), false);
    assert!(
        !strict.status.success(),
        "plain --fix must exit non-zero while a residual uncovered link remains; stderr=\n{}",
        String::from_utf8_lossy(&strict.stderr)
    );
}

/// `wiki scaffold` no longer exists; `--no-mesh` no longer exists.
#[test]
fn scaffold_subcommand_and_no_mesh_flag_are_removed() {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);

    let scaffold = Command::new(bin)
        .args(["scaffold", "**/*.md"])
        .current_dir(tmp.path())
        .output()
        .expect("run wiki");
    assert!(
        !scaffold.status.success(),
        "`wiki scaffold` must error as an unknown subcommand"
    );

    let no_mesh = Command::new(bin)
        .args(["check", "--no-mesh"])
        .current_dir(tmp.path())
        .output()
        .expect("run wiki");
    assert!(
        !no_mesh.status.success(),
        "`--no-mesh` must error as an unknown flag"
    );
}

/// `--fix-dry-run` previews the meshes that WOULD be created without mutating
/// `.mesh/`.
#[test]
fn fix_dry_run_previews_meshes_without_mutating() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// lib\n// line2\n// line3\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [lib](../src/lib.rs#L1-L3) for details.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args([
            "check",
            "--fix",
            "--fix-dry-run",
            "--no-exit-code",
            "**/*.md",
        ])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki check --fix-dry-run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "dry-run must exit 0 with --no-exit-code"
    );
    assert!(
        stdout.contains("create mesh: .mesh/"),
        "dry-run must preview a mesh that would be created; stdout=\n{stdout}"
    );

    // Nothing on disk was mutated.
    assert!(
        !root.join(".mesh").exists() || walkdir(&root.join(".mesh")).is_empty(),
        "dry-run must not create any mesh files"
    );
}

/// A line-ranged fragment link into a gitignored (generated) file must NOT be
/// anchored: git-mesh cannot anchor a path git never sees, so the engine drops
/// it with an advisory and creates no mesh for the gitignored path. With
/// --no-exit-code the run exits 0.
#[test]
fn skips_gitignored_anchor_target() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/generated.ts"),
        "// l1\n// l2\n// l3\n// l4\n// l5\n",
    )
    .unwrap();
    std::fs::write(root.join(".gitignore"), "src/generated.ts\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\n\
         Tokens validate against [keys](../src/generated.ts#L1-L5).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let output = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "must exit 0 with --no-exit-code; stderr=\n{stderr}"
    );
    // No applied mesh — the only link targets a gitignored path.
    assert!(
        stdout.lines().all(|l| l.is_empty()),
        "must not apply any mesh for a gitignored-only page; stdout=\n{stdout}"
    );
    // No mesh on disk anchors the gitignored path.
    let mesh_dir = root.join(".mesh");
    if mesh_dir.exists() {
        for entry in walkdir(&mesh_dir) {
            let body = std::fs::read_to_string(&entry).unwrap_or_default();
            assert!(
                !body.contains("src/generated.ts"),
                "a mesh anchors the gitignored path: {}\n{body}",
                entry.display()
            );
        }
    }
}

/// A section co-citing a tracked file AND a gitignored file still gets a mesh
/// covering the *tracked* anchor; only the gitignored anchor is stripped.
#[test]
fn keeps_tracked_anchor_when_co_cited_with_gitignored() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/real.ts"),
        "export const A = 1;\nexport const B = 2;\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/gen.ts"),
        "export const G = 1;\nexport const H = 2;\n",
    )
    .unwrap();
    std::fs::write(root.join(".gitignore"), "src/gen.ts\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\n# Page\n\n\
         Cites [real](../src/real.ts#L1-L2) and [gen](../src/gen.ts#L1-L2).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let output = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "must exit 0 with --no-exit-code; stderr=\n{stderr}"
    );

    let applied: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        applied.len(),
        1,
        "expected exactly one applied mesh; stdout=\n{stdout}"
    );
    let body = std::fs::read_to_string(root.join(applied[0])).expect("read applied mesh");
    assert!(
        body.contains("src/real.ts"),
        "applied mesh must anchor the tracked file:\n{body}"
    );
    assert!(
        !body.contains("src/gen.ts"),
        "applied mesh must NOT anchor the gitignored file:\n{body}"
    );
    assert!(
        stderr.contains("src/gen.ts"),
        "expected stripped-anchor advisory on stderr; stderr=\n{stderr}"
    );
}

/// An over-range fragment anchor (code shrank under the link) is fixable wiki
/// drift, not a fatal error: the engine drops the draft with a named advisory
/// and (with --no-exit-code) the run exits 0.
#[test]
fn over_range_anchor_dropped_with_named_advisory() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// l1\n// l2\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [x](../src/lib.rs#L1-L999) for details.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let output = check_fix_print_applied(&root.join("wiki"), true);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "over-range anchor must not lock the run with --no-exit-code; stderr=\n{stderr}"
    );
    assert!(
        stderr.contains("Skipped mesh")
            && stderr.contains("invalid anchor")
            && stderr.contains("src/lib.rs#L1-L999")
            && stderr.contains("end exceeds file line count 2"),
        "advisory must name the offending anchor and reason on stderr; stderr=\n{stderr}"
    );
}

/// Running `wiki check --fix` twice on an unchanged tree produces identical
/// mesh content (git-mesh idempotency) and exits 0 both times.
#[test]
fn fix_mesh_creation_is_idempotent() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// lib\n// line2\n// line3\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [lib](../src/lib.rs#L1-L3) for details.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let first = check_fix_print_applied(&root.join("wiki"), false);
    assert!(first.status.success(), "first run must exit 0");
    let mesh_dir = root.join(".mesh");
    let first_content = walkdir_mesh(&mesh_dir);

    let second = check_fix_print_applied(&root.join("wiki"), false);
    assert!(second.status.success(), "second run must exit 0");
    let second_content = walkdir_mesh(&mesh_dir);

    assert_eq!(
        first_content, second_content,
        "mesh content changed between two identical fix runs (not idempotent)"
    );
}

/// When a fragment link references a source path that does not exist on disk,
/// the mesh is dropped with an advisory and no mesh is created for it.
#[test]
fn drops_mesh_with_missing_anchor_path() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/present.rs"),
        "// l1\n// l2\n// l3\n// l4\n// l5\n",
    )
    .unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\n\
         ## Section A\n\nSee [present](../src/present.rs#L1-L5).\n\n\
         ## Section B\n\nSee [missing](../src/missing.rs#L1-L5).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let output = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "must exit 0 with --no-exit-code; stderr=\n{stderr}"
    );

    // The present-anchor mesh was created.
    let applied: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        applied.iter().any(|l| {
            std::fs::read_to_string(root.join(l))
                .unwrap_or_default()
                .contains("src/present.rs#L1-L5")
        }),
        "present-anchor mesh must be created; stdout=\n{stdout}"
    );
    // The missing-anchor mesh is dropped with an advisory naming the path.
    assert!(
        stderr.contains("Skipped mesh") && stderr.contains("src/missing.rs"),
        "advisory for dropped missing-path mesh must appear on stderr; stderr=\n{stderr}"
    );
}

/// The engine treats existing meshes as the anchor for a wiki section: a new
/// code link in a section whose mesh already exists extends that mesh rather
/// than creating a new one.
#[test]
fn extends_existing_section_mesh_with_new_code_links() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/extend.ts"), "// extend\n").unwrap();
    // v1: cite the file once and create its mesh.
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Billing\nsummary: bill\n---\n\n## Extends existing\n\nSee [extend](../src/extend.ts#L1-L1).\n",
    )
    .unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "v1"]);

    let first = check_fix_print_applied(&root.join("wiki"), false);
    assert!(
        first.status.success(),
        "first fix run must exit 0: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_applied: Vec<String> = String::from_utf8_lossy(&first.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|s| s.to_string())
        .collect();
    assert_eq!(first_applied.len(), 1, "expected one mesh created on v1");
    let mesh_rel = first_applied[0].clone();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "mesh"]);

    // v2: add a second code link in the same section → extend the existing mesh.
    std::fs::write(root.join("src/more.ts"), "// more\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Billing\nsummary: bill\n---\n\n## Extends existing\n\n\
         See [extend](../src/extend.ts#L1-L1) and [more](../src/more.ts#L1-L1).\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "v2"]);

    let second = check_fix_print_applied(&root.join("wiki"), false);
    assert!(
        second.status.success(),
        "extend run must exit 0: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // The existing mesh now also carries the new code anchor.
    let body = std::fs::read_to_string(root.join(&mesh_rel)).expect("read mesh");
    assert!(
        body.contains("src/more.ts#L1-L1"),
        "existing mesh must be extended with the new code anchor; got:\n{body}"
    );
}

/// A pre-existing mesh whose name is a strict ANCESTOR of a slug the page would
/// generate is renamed out of the way, the previously-colliding draft is then
/// created, and the run exits 0. The renamed blocker's NEW path is staged via
/// `--print-applied`.
#[test]
fn renames_blocker_and_keeps_others() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    if !git_mesh_add_sees_uncommitted_rename() {
        eprintln!(
            "skipping: this git-mesh's add collision check is HEAD-based — \
             an uncommitted in-run rename cannot free the slug"
        );
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki/arch/scaff")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// lib\n// line2\n// line3\n").unwrap();
    std::fs::write(
        root.join("wiki/arch/scaff/page.md"),
        "---\ntitle: Scaff Page\nsummary: Colliding page.\n---\n\n\
         ## Helper\n\nSee [lib](../../../src/lib.rs#L1-L3) here.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    let mesh_status = Command::new("git")
        .args(["mesh", "add", "wiki/arch/scaff", "src/lib.rs#L1-L1"])
        .current_dir(root)
        .status()
        .expect("git mesh add available");
    assert!(mesh_status.success(), "pre-create blocker mesh failed");
    let blocker = root.join(".mesh/wiki/arch/scaff");
    let blocker_before = std::fs::read_to_string(&blocker).unwrap();

    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let output = check_fix_print_applied(&root.join("wiki"), false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "must exit 0 despite the slug-path collision; stderr=\n{stderr}"
    );

    // The performed-rename advisory is on stderr.
    assert!(
        stderr.contains(
            "Renamed mesh `wiki/arch/scaff` -> `wiki/arch/scaff/index` to free path for `wiki/arch/scaff/scaff-page/helper`"
        ),
        "expected the blocker-rename advisory on stderr; stderr=\n{stderr}"
    );

    // The blocker's content is preserved byte-equivalent at its new path.
    let renamed = root.join(".mesh/wiki/arch/scaff/index");
    assert!(
        renamed.is_file(),
        ".mesh/wiki/arch/scaff/index must exist (renamed blocker)"
    );
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), blocker_before);

    // The previously-colliding draft was created.
    let new_mesh = root.join(".mesh/wiki/arch/scaff/scaff-page/helper");
    assert!(new_mesh.is_file(), "the freed slug must apply");

    // Both the renamed-blocker NEW path and the new mesh appear on stdout.
    let applied: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        applied.contains(&".mesh/wiki/arch/scaff/index"),
        "renamed-blocker NEW path must be in the applied stage list; stdout=\n{stdout}"
    );
    assert!(
        applied.contains(&".mesh/wiki/arch/scaff/scaff-page/helper"),
        "the freed mesh must be in the applied stage list; stdout=\n{stdout}"
    );
}

/// `--fix-dry-run` reports the planned blocker rename and mutates nothing.
#[test]
fn fix_dry_run_reports_planned_rename_without_mutating() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki/arch/scaff")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// lib\n// line2\n// line3\n").unwrap();
    std::fs::write(
        root.join("wiki/arch/scaff/page.md"),
        "---\ntitle: Scaff Page\nsummary: Colliding page.\n---\n\n\
         ## Helper\n\nSee [lib](../../../src/lib.rs#L1-L3) here.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    let mesh_status = Command::new("git")
        .args(["mesh", "add", "wiki/arch/scaff", "src/lib.rs#L1-L1"])
        .current_dir(root)
        .status()
        .expect("git mesh add available");
    assert!(mesh_status.success(), "pre-create blocker mesh failed");
    let blocker = root.join(".mesh/wiki/arch/scaff");
    let blocker_before = std::fs::read_to_string(&blocker).unwrap();

    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args([
            "check",
            "--fix",
            "--fix-dry-run",
            "--no-exit-code",
            "**/*.md",
        ])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki check --fix-dry-run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "dry-run must exit 0 with --no-exit-code"
    );
    assert!(
        stdout.contains(
            "Would rename mesh `wiki/arch/scaff` -> `wiki/arch/scaff/index` to free path for `wiki/arch/scaff/scaff-page/helper`."
        ),
        "expected the 'Would rename' advisory on stdout; stdout=\n{stdout}"
    );

    // Nothing on disk changed.
    assert!(blocker.is_file(), "dry-run must not move the blocker");
    assert_eq!(std::fs::read_to_string(&blocker).unwrap(), blocker_before);
    assert!(
        !root.join(".mesh/wiki/arch/scaff/index").exists(),
        "dry-run must not create the rename target"
    );
}

// ── Mesh cleanup (delete) integration tests ───────────────────────────────────

/// Helper: run `wiki check --fix-dry-run` from `cwd`. Returns process output.
fn check_fix_dry_run(cwd: &Path) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    Command::new(bin)
        .args([
            "check",
            "--fix",
            "--fix-dry-run",
            "--no-exit-code",
            "**/*.md",
        ])
        .current_dir(cwd)
        .output()
        .expect("run wiki check --fix-dry-run")
}

/// Round-trip clean: scaffold a page's mesh, delete the page, run `--fix
/// --print-applied` ⇒ scaffold mesh is gone, its path printed on stdout, and a
/// follow-up `wiki check` reports no residual diagnostics.
#[test]
fn cleanup_round_trip_deletes_orphan_mesh() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// l1\n// l2\n// l3\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [lib](../src/lib.rs#L1-L3) for details.\n",
    )
    .unwrap();
    // Keep a second page so the corpus is never empty after deletion.
    std::fs::write(
        root.join("wiki/other.md"),
        "---\ntitle: Other\nsummary: Other page.\n---\n\n# Other\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // First pass: create the scaffold mesh.
    let first = check_fix_print_applied(&root.join("wiki"), true);
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        first.status.success(),
        "first pass must succeed; stderr=\n{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let mesh_path: &str = first_stdout
        .lines()
        .find(|l| l.starts_with(".mesh/"))
        .expect("first pass must create a mesh");
    assert!(
        root.join(mesh_path).is_file(),
        "mesh must exist after creation"
    );

    // Commit what was created.
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "mesh created"]);

    // Delete the wiki page.
    std::fs::remove_file(root.join("wiki/page.md")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "page deleted"]);

    // Second pass: cleanup orphan mesh.
    let second = check_fix_print_applied(&root.join("wiki"), true);
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    let second_stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        second.status.success(),
        "cleanup pass must succeed; stderr=\n{second_stderr}"
    );

    // The deleted mesh path must appear on stdout.
    assert!(
        second_stdout.lines().any(|l| l == mesh_path),
        "deleted mesh path must appear on stdout; stdout=\n{second_stdout}"
    );
    // The mesh file must be gone.
    assert!(
        !root.join(mesh_path).exists(),
        "mesh file must be deleted after cleanup"
    );

    // Follow-up check must report no residual diagnostics.
    let bin = env!("CARGO_BIN_EXE_wiki");
    let followup = Command::new(bin)
        .args(["check", "**/*.md"])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki check");
    assert!(
        followup.status.success(),
        "follow-up check must be clean; stderr=\n{}",
        String::from_utf8_lossy(&followup.stderr)
    );
}

/// `--fix-dry-run` lists the would-delete mesh and mutates nothing.
#[test]
fn cleanup_dry_run_previews_deletion_without_mutating() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// l1\n// l2\n// l3\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [lib](../src/lib.rs#L1-L3) for details.\n",
    )
    .unwrap();
    // Keep a second page so the corpus is never empty after deletion.
    std::fs::write(
        root.join("wiki/other.md"),
        "---\ntitle: Other\nsummary: Other page.\n---\n\n# Other\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // Create the scaffold mesh.
    let create = check_fix_print_applied(&root.join("wiki"), true);
    assert!(create.status.success());
    let mesh_path = String::from_utf8_lossy(&create.stdout)
        .lines()
        .find(|l| l.starts_with(".mesh/"))
        .expect("mesh must be created")
        .to_string();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "mesh"]);

    // Delete the wiki page and commit.
    std::fs::remove_file(root.join("wiki/page.md")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "page deleted"]);

    // Dry run: must preview deletion, not delete.
    let dry = check_fix_dry_run(&root.join("wiki"));
    let dry_stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(
        dry.status.success(),
        "dry-run must succeed; stderr=\n{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(
        dry_stdout.contains(&format!("delete mesh: {mesh_path}")),
        "dry-run must preview deletion; stdout=\n{dry_stdout}"
    );
    // Mesh file must still exist.
    assert!(
        root.join(&mesh_path).is_file(),
        "dry-run must not delete mesh file"
    );
}

/// Curated `why` orphan ⇒ advisory on stderr, mesh untouched.
#[test]
fn cleanup_curated_why_produces_advisory_not_deletion() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mesh_dir = root.join(".mesh");
    std::fs::create_dir_all(&mesh_dir).unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();

    // Hand-write a curated mesh (with why prose) whose page is absent.
    std::fs::write(
        mesh_dir.join("curated-flow"),
        "wiki/gone.md#L1-L5 sha256:abc\n\nCurated description of the flow.\n",
    )
    .unwrap();
    // wiki/gone.md does NOT exist.

    // Need a real git repo so check can run.
    std::fs::write(
        root.join("wiki/other.md"),
        "---\ntitle: Other\nsummary: Other page.\n---\n\n# Other\n",
    )
    .unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let out = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Mesh must not be deleted.
    assert!(
        mesh_dir.join("curated-flow").is_file(),
        "curated mesh must not be deleted"
    );
    // Advisory must appear on stderr.
    assert!(
        stderr.contains("curated why prose") || stderr.contains("curated-flow"),
        "expected curated-why advisory on stderr; stderr=\n{stderr}"
    );
    // Mesh path must NOT appear on stdout.
    assert!(
        !stdout.contains("curated-flow"),
        "curated mesh path must not appear on stdout; stdout=\n{stdout}"
    );
}

/// Multi-page mesh with one page gone ⇒ advisory on stderr, mesh untouched.
#[test]
fn cleanup_multi_page_mesh_produces_advisory_not_deletion() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mesh_dir = root.join(".mesh");
    std::fs::create_dir_all(&mesh_dir).unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();

    // Multi-page scaffold mesh: two .md paths, one gone.
    std::fs::write(
        mesh_dir.join("shared-flow"),
        "wiki/gone.md#L1-L3 sha256:aaa\nwiki/present.md#L2-L4 sha256:bbb\n\n",
    )
    .unwrap();
    // wiki/present.md exists; wiki/gone.md does not.
    std::fs::write(
        root.join("wiki/present.md"),
        "---\ntitle: Present\nsummary: Present page.\n---\n\n# Present\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    let out = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Mesh must not be deleted.
    assert!(
        mesh_dir.join("shared-flow").is_file(),
        "multi-page mesh must not be deleted"
    );
    // Advisory must appear on stderr.
    assert!(
        stderr.contains("other pages") || stderr.contains("shared-flow"),
        "expected multi-page advisory on stderr; stderr=\n{stderr}"
    );
    // Mesh path must NOT appear on stdout.
    assert!(
        !stdout.contains("shared-flow"),
        "multi-page mesh path must not appear on stdout; stdout=\n{stdout}"
    );
}

/// Frontmatter demotion (file still on disk) ⇒ no deletion.
#[test]
fn cleanup_no_deletion_when_file_present() {
    if !git_mesh_available() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "// l1\n// l2\n// l3\n").unwrap();
    std::fs::write(
        root.join("wiki/page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [lib](../src/lib.rs#L1-L3) for details.\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);

    // Create scaffold mesh.
    let create = check_fix_print_applied(&root.join("wiki"), true);
    assert!(create.status.success());
    let mesh_path = String::from_utf8_lossy(&create.stdout)
        .lines()
        .find(|l| l.starts_with(".mesh/"))
        .expect("mesh must be created")
        .to_string();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "mesh"]);

    // Demote page to plain markdown (remove frontmatter but keep the file).
    std::fs::write(root.join("wiki/page.md"), "# Plain markdown\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "demoted"]);

    // Run fix: no deletion because the file still exists.
    let out = check_fix_print_applied(&root.join("wiki"), true);
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Mesh must still exist.
    assert!(
        root.join(&mesh_path).is_file(),
        "mesh must not be deleted when file is present"
    );
    // Deleted mesh path must NOT appear on stdout.
    assert!(
        !stdout.contains(&mesh_path),
        "mesh path must not appear on stdout for demoted-but-present page; stdout=\n{stdout}"
    );
}
