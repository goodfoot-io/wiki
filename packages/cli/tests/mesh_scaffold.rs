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
        .args(["scaffold", "**/*.md"])
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
        .args(["scaffold", "**/*.md"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki scaffold failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Because a clean file with links follows, the advisory header is used.
    assert!(
        stdout.starts_with("Some wiki pages could not be parsed and were skipped:\n"),
        "expected advisory parse-error header at start, got:\n{stdout}"
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
        .args(["scaffold", "**/*.md"])
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

/// JSON mode emits `{ schemaVersion: 1, parseErrors: [], pages: [...] }` shape.
#[test]
fn mesh_scaffold_json_shape_and_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // Page with a heading chain where top equals title (trim positive).
    std::fs::write(
        wiki_dir.join("billing.md"),
        "---\ntitle: Billing\n---\n\n# Billing\n\n## Charge handler\n\nThe handler processes charges. See [charge](src/charge.rs#L1-L5) for details.\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("charge.rs"), "// charge\n").unwrap();

    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(output.status.success(), "wiki scaffold --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON output");

    // schemaVersion must be 1.
    assert_eq!(v["schemaVersion"], 1, "schemaVersion must be 1:\n{stdout}");

    // parseErrors must be present and empty.
    assert!(v["parseErrors"].is_array(), "parseErrors must be array:\n{stdout}");
    assert!(v["parseErrors"].as_array().unwrap().is_empty(), "parseErrors must be empty:\n{stdout}");

    // pages must be present and non-empty.
    assert!(v["pages"].is_array(), "pages must be array:\n{stdout}");
    let pages = v["pages"].as_array().unwrap();
    assert!(!pages.is_empty(), "pages must be non-empty:\n{stdout}");

    let page = &pages[0];
    assert_eq!(page["path"], "wiki/billing.md");
    assert_eq!(page["title"], "Billing");

    let meshes = page["meshes"].as_array().unwrap();
    assert!(!meshes.is_empty());
    let mesh = &meshes[0];

    // headingChain must be trimmed (Billing dropped, Charge handler kept).
    let chain = mesh["headingChain"].as_array().unwrap();
    assert_eq!(chain.len(), 1, "leading 'Billing' should be trimmed, got chain: {chain:?}");
    assert_eq!(chain[0], "Charge handler");

    // sectionOpening field must be absent — heading chain alone identifies the section.
    assert!(mesh.get("sectionOpening").is_none(), "sectionOpening field must be absent:\n{stdout}");

    // anchors must be structured objects, with the page section anchor first.
    let anchors = mesh["anchors"].as_array().unwrap();
    assert!(anchors.len() >= 2, "anchors must contain page section + targets:\n{stdout}");
    assert_eq!(anchors[0]["path"], "wiki/billing.md", "anchors[0] must be the page section anchor:\n{stdout}");
    assert!(anchors[0]["startLine"].is_number(), "anchor.startLine must be number:\n{stdout}");
    assert!(anchors[0]["endLine"].is_number(), "anchor.endLine must be number:\n{stdout}");
    assert_eq!(anchors[1]["path"], "src/charge.rs", "anchors[1] must be the target:\n{stdout}");

    // Legacy fields must be absent.
    assert!(mesh["name"].is_null(), "legacy 'name' field must be absent:\n{stdout}");
    assert!(mesh["why"].is_null(), "legacy 'why' field must be absent:\n{stdout}");
    assert!(mesh["wikiFile"].is_null(), "legacy 'wikiFile' field must be absent:\n{stdout}");
    assert!(mesh["anchor"].is_null(), "legacy 'anchor' string field must be absent:\n{stdout}");
}

/// JSON mode with empty corpus still emits the structured object, not `[]`.
#[test]
fn mesh_scaffold_json_empty_corpus_structured_output() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // A valid file with no fragment links → empty corpus.
    std::fs::write(
        wiki_dir.join("page.md"),
        "---\ntitle: Empty\n---\n\nNo links here.\n",
    )
    .unwrap();

    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(output.status.success(), "wiki scaffold --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    // Must be an object (not `[]`).
    assert!(v.is_object(), "empty corpus must emit object, not array:\n{stdout}");
    assert_eq!(v["schemaVersion"], 1);
    assert!(v["parseErrors"].is_array());
    assert!(v["pages"].is_array());
    assert!(v["pages"].as_array().unwrap().is_empty());
}

/// JSON mode — parse errors appear in `parseErrors[]` with correct category tags.
#[test]
fn mesh_scaffold_json_parse_errors_in_output() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // Category 1: NoFrontmatter.
    std::fs::write(wiki_dir.join("no_fm.md"), "# No frontmatter\n").unwrap();
    // Category 2: MissingTitle.
    std::fs::write(wiki_dir.join("missing_title.md"), "---\nsummary: x\n---\n\nbody\n").unwrap();
    // Category 3: EmptyTitle.
    std::fs::write(wiki_dir.join("empty_title.md"), "---\ntitle:\n---\n\nbody\n").unwrap();
    // Clean file with no links (so all_inputs.is_empty() → but we want the parse_errors path).
    std::fs::write(wiki_dir.join("clean.md"), "---\ntitle: Clean\n---\n\nNo links.\n").unwrap();

    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(output.status.success(), "wiki scaffold --json failed: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let errors = v["parseErrors"].as_array().unwrap();
    assert!(!errors.is_empty(), "parseErrors must be non-empty:\n{stdout}");

    // Check category tags are snake_case.
    let categories: Vec<&str> = errors
        .iter()
        .map(|e| e["category"].as_str().unwrap())
        .collect();
    assert!(categories.contains(&"no_frontmatter"), "no_frontmatter category missing: {categories:?}");
    assert!(categories.contains(&"missing_title"), "missing_title category missing: {categories:?}");
    assert!(categories.contains(&"empty_title"), "empty_title category missing: {categories:?}");

    // Each error must have path and message.
    for e in errors {
        assert!(e["path"].is_string(), "error.path must be string");
        assert!(e["message"].is_string(), "error.message must be string");
        assert!(e["category"].is_string(), "error.category must be string");
    }
}

/// JSON mode — top-of-file link (no heading above) has empty headingChain.
#[test]
fn mesh_scaffold_json_top_of_file_link_empty_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    std::fs::write(
        wiki_dir.join("page.md"),
        "---\ntitle: My Page\n---\n\nSee [x](src/x.rs#L1-L2) at the top.\n\n# Heading below\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("x.rs"), "// x\n").unwrap();

    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let mesh = &v["pages"][0]["meshes"][0];
    let chain = mesh["headingChain"].as_array().unwrap();
    assert!(chain.is_empty(), "top-of-file link must have empty headingChain, got: {chain:?}");
}

/// JSON mode — a file in `parseErrors[]` must NOT appear in `pages[]` (disjoint
/// schema). A file with malformed frontmatter that also contains fragment links
/// should land in parseErrors and nowhere else.
#[test]
fn mesh_scaffold_json_parse_error_page_not_in_pages() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // File with missing title but contains a fragment link — should be in parseErrors only.
    std::fs::write(
        wiki_dir.join("bad.md"),
        "---\nsummary: x\n---\n\nSee [x](src/x.rs#L1-L2) for details.\n",
    )
    .unwrap();
    // A clean file so pages[] is non-empty.
    std::fs::write(
        wiki_dir.join("clean.md"),
        "---\ntitle: Clean\n---\n\nSee [y](src/y.rs#L1-L2) for details.\n",
    )
    .unwrap();

    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("x.rs"), "// x\n").unwrap();
    std::fs::write(src_dir.join("y.rs"), "// y\n").unwrap();

    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(tmp.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki scaffold --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    // bad.md must be in parseErrors.
    let errors = v["parseErrors"].as_array().unwrap();
    assert!(
        errors.iter().any(|e| e["path"].as_str().is_some_and(|p| p.contains("bad.md"))),
        "bad.md must appear in parseErrors:\n{stdout}"
    );

    // bad.md must NOT be in pages.
    let pages = v["pages"].as_array().unwrap();
    assert!(
        !pages.iter().any(|p| p["path"].as_str().is_some_and(|pp| pp.contains("bad.md"))),
        "bad.md must not appear in pages[]:\n{stdout}"
    );

    // clean.md must be in pages.
    assert!(
        pages.iter().any(|p| p["path"].as_str().is_some_and(|pp| pp.contains("clean.md"))),
        "clean.md must appear in pages[]:\n{stdout}"
    );
}

/// JSON mode — unreadable (non-UTF-8) file appears in `parseErrors[]` with
/// `category == "unreadable"` and does NOT appear in `pages[]`. Exit code is 0.
#[test]
fn mesh_scaffold_json_unreadable_file_in_parse_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();
    std::fs::write(wiki_dir.join("wiki.toml"), "").unwrap();

    // A valid file with a fragment link so pages[] is non-empty.
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
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");

    assert!(
        output.status.success(),
        "expected exit 0 for unreadable file in JSON mode, got failure.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    // parseErrors must contain the unreadable file with category "unreadable".
    let errors = v["parseErrors"].as_array().unwrap();
    let unreadable_entry = errors.iter().find(|e| {
        e["path"].as_str().is_some_and(|p| p.contains("unreadable.md"))
    });
    assert!(
        unreadable_entry.is_some(),
        "unreadable.md must appear in parseErrors:\n{stdout}"
    );
    assert_eq!(
        unreadable_entry.unwrap()["category"],
        "unreadable",
        "category must be 'unreadable':\n{stdout}"
    );

    // The unreadable file must NOT appear in pages[].
    let pages = v["pages"].as_array().unwrap();
    let bad_page = pages.iter().find(|p| {
        p["path"].as_str().is_some_and(|pp| pp.contains("unreadable.md"))
    });
    assert!(
        bad_page.is_none(),
        "unreadable.md must not appear in pages[]:\n{stdout}"
    );
}

/// `wiki scaffold <float>` against a `*.wiki.md` file outside any wiki root
/// must succeed. Today the multi-namespace dispatch evaluates the path
/// against every peer's discovery glob; peers whose roots don't enclose the
/// float report "no wiki pages found" and the run fails. Owning-ns routing
/// should send the float to its single owning namespace and skip the others.
#[test]
fn mesh_scaffold_handles_float_outside_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("foo")).unwrap();
    std::fs::create_dir_all(root.join("floats")).unwrap();
    std::fs::write(root.join("wiki/wiki.toml"), "[peers]\nfoo = \"../foo\"\n").unwrap();
    std::fs::write(
        root.join("wiki/index.md"),
        "---\ntitle: Default Index\nsummary: D.\n---\nHi.\n",
    )
    .unwrap();
    std::fs::write(root.join("foo/wiki.toml"), "namespace = \"foo\"\n").unwrap();
    std::fs::write(
        root.join("foo/index.md"),
        "---\ntitle: Foo Index\nsummary: F.\n---\nHi.\n",
    )
    .unwrap();
    // Untagged float — owned by default. Body has a fragment link so the
    // scaffold has something to emit.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/code.rs"), "fn x() {}\n").unwrap();
    std::fs::write(
        root.join("floats/notes.wiki.md"),
        "---\ntitle: Notes\nsummary: N.\n---\nSee [code](../src/code.rs#L1-L1).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(
        root,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "add",
            "-A",
        ],
    );
    git(
        root,
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
        .args(["scaffold", "../floats/notes.wiki.md"])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki scaffold");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "scaffold against an out-of-root float must succeed; \
         status={:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status
    );
    // Scaffold must emit a `git mesh add` line that anchors the float to its
    // fragment-link target — if the run silently became a no-op the success
    // assertion alone would not catch it.
    assert!(
        stdout.contains("git mesh add") && stdout.contains("src/code.rs#L1-L1"),
        "scaffold must emit a mesh entry covering the float's fragment link; \
         stdout: {stdout}\nstderr: {stderr}"
    );
}

/// Slugs for pages in a *named-namespace* wiki must start with
/// `wiki/<namespace>/`, and the namespace name must not appear a second time
/// even when the wiki root directory happens to match the namespace.
#[test]
fn mesh_scaffold_namespaced_wiki_slug_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A named-namespace wiki at `mesh/` (namespace == directory name, as is
    // typical in this repo). The fact that the dir and namespace share a name
    // is exactly the case where the no-repeat rule has to kick in.
    std::fs::create_dir_all(root.join("mesh/sub")).unwrap();
    std::fs::write(root.join("mesh/wiki.toml"), "namespace = \"mesh\"\n").unwrap();
    std::fs::write(
        root.join("mesh/charge.md"),
        "---\ntitle: Charge\nsummary: c\n---\n\n## Handler\n\nSee [code](../src/charge.rs#L1-L5).\n",
    )
    .unwrap();
    std::fs::write(
        root.join("mesh/sub/leaf.md"),
        "---\ntitle: Leaf\nsummary: l\n---\n\n## Inner\n\nSee [bit](../../src/leaf.rs#L1-L2).\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/charge.rs"), "// charge\n").unwrap();
    std::fs::write(root.join("src/leaf.rs"), "// leaf\n").unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md"])
        .current_dir(root.join("mesh"))
        .output()
        .expect("run wiki scaffold");
    assert!(
        output.status.success(),
        "scaffold failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Top-level page in the namespaced wiki → wiki/mesh/<noun>.
    assert!(
        stdout.contains("git mesh add wiki/mesh/handler"),
        "expected wiki/mesh/handler slug; got:\n{stdout}"
    );
    // No double-namespace segment from the wiki root dir matching the
    // namespace name.
    assert!(
        !stdout.contains("wiki/mesh/mesh/"),
        "namespace must not be repeated in slug; got:\n{stdout}"
    );

    // Subdir within the namespaced wiki → wiki/mesh/sub/<noun>.
    assert!(
        stdout.contains("git mesh add wiki/mesh/sub/inner"),
        "expected wiki/mesh/sub/inner slug; got:\n{stdout}"
    );
}

/// Float `.wiki.md` pages outside any `wiki.toml` tree must use their
/// frontmatter `namespace` field to choose the slug prefix:
///   * no `namespace` → `wiki/<noun>` (default namespace),
///   * `namespace: foo` → `wiki/foo/<noun>` (named peer),
/// regardless of where on disk the file sits.
#[test]
fn mesh_scaffold_float_uses_frontmatter_namespace() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Default-namespace wiki anchor so `WikiConfig::load` has something to find.
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(root.join("wiki/wiki.toml"), "").unwrap();
    std::fs::write(
        root.join("wiki/index.md"),
        "---\ntitle: Index\nsummary: i\n---\n\nNo links.\n",
    )
    .unwrap();
    // Named-namespace wiki for the float to opt into.
    std::fs::create_dir_all(root.join("mesh")).unwrap();
    std::fs::write(root.join("mesh/wiki.toml"), "namespace = \"mesh\"\n").unwrap();
    std::fs::write(
        root.join("mesh/anchor.md"),
        "---\ntitle: Anchor\nsummary: a\n---\n\nNo links.\n",
    )
    .unwrap();

    // Two floats in `src/`: one default (no fm namespace), one tagged `mesh`.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/x.rs"), "// x\n").unwrap();
    std::fs::write(root.join("src/y.rs"), "// y\n").unwrap();
    std::fs::write(
        root.join("src/default.wiki.md"),
        "---\ntitle: Default Notes\nsummary: d\n---\n\n## Plain\n\nSee [code](x.rs#L1-L2).\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/tagged.wiki.md"),
        "---\ntitle: Tagged Notes\nsummary: t\nnamespace: mesh\n---\n\n## Peer\n\nSee [code](y.rs#L1-L2).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md", "**/*.wiki.md"])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki scaffold");
    assert!(
        output.status.success(),
        "scaffold failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The default-ns float drops its parent directory and gets the bare
    // `wiki/` prefix.
    assert!(
        stdout.contains("git mesh add wiki/plain"),
        "expected wiki/plain slug for untagged float; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("wiki/src/"),
        "default-ns float must not carry the on-disk `src/` segment; got:\n{stdout}"
    );

    // The frontmatter-tagged float adopts the peer namespace.
    assert!(
        stdout.contains("git mesh add wiki/mesh/peer"),
        "expected wiki/mesh/peer slug for `namespace: mesh` float; got:\n{stdout}"
    );
}

/// When the base slug a scaffold run would emit already names a mesh
/// committed in the repo, the collision resolver picks a semantically
/// disambiguated slug — prepending the page title or a parent heading —
/// instead of falling straight through to a numeric suffix.
#[test]
fn mesh_scaffold_renames_on_existing_mesh_collision() {
    // Skip silently when `git-mesh` is not on PATH. The unit tests cover the
    // resolver logic; this integration test only exercises the live probe.
    if Command::new("git-mesh").arg("--version").output().is_err() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(root.join("wiki/wiki.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/charge.ts"), "// charge\n").unwrap();
    // Page's frontmatter title is "Billing"; the deepest heading is
    // "Charge handler", which yields the colliding base slug
    // `wiki/charge-handler`.
    std::fs::write(
        root.join("wiki/billing.md"),
        "---\ntitle: Billing\nsummary: bill\n---\n\n## Charge handler\n\nSee [code](../src/charge.ts#L1-L1).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    // Pre-stage a mesh at the exact slug scaffold would otherwise pick.
    let stage = |args: &[&str]| {
        let status = Command::new("git")
            .arg("mesh")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git mesh available");
        assert!(
            status.status.success(),
            "git mesh {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    stage(&["add", "wiki/charge-handler", "src/charge.ts#L1-L1"]);
    stage(&["why", "wiki/charge-handler", "-m", "pre-existing"]);
    stage(&["commit", "wiki/charge-handler"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md"])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki scaffold");
    assert!(
        output.status.success(),
        "scaffold failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Base slug is taken — the resolver must rename. With no heading-chain
    // ancestors (the section is top-level), the page's frontmatter title
    // ("Billing") is the first available semantic qualifier.
    assert!(
        !stdout.contains("git mesh add wiki/charge-handler "),
        "scaffold reused the existing slug; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("git mesh add wiki/charge-handler-2"),
        "scaffold fell through to a digit suffix instead of a semantic rename; got:\n{stdout}"
    );
    assert!(
        stdout.contains("git mesh add wiki/billing/charge-handler"),
        "expected resolver to prepend the page title; got:\n{stdout}"
    );
}

/// `wiki scaffold` must skip emitting `git mesh add` blocks for fragment
/// links whose `(code-path, start, end) ↔ wiki-page` anchor pair is already
/// covered by an existing mesh — the same predicate `wiki check`'s
/// `mesh_uncovered` rule applies via
/// [`collect_mesh_diagnostics`](../src/commands/mesh_coverage.rs).
///
/// Reproduction layout: one wiki page with two fragment links — one whose
/// anchor pair is already covered by a pre-existing mesh, one that is not.
/// Only the uncovered link should appear in the scaffold output.
#[test]
fn mesh_scaffold_skips_already_covered_fragment_links() {
    if Command::new("git-mesh").arg("--version").output().is_err() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(root.join("wiki/wiki.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/covered.ts"), "// covered\n").unwrap();
    std::fs::write(root.join("src/uncovered.ts"), "// uncovered\n").unwrap();
    std::fs::write(
        root.join("wiki/billing.md"),
        "---\ntitle: Billing\nsummary: bill\n---\n\n\
         ## Covered section\n\n\
         See [covered](../src/covered.ts#L1-L1).\n\n\
         ## Uncovered section\n\n\
         See [uncovered](../src/uncovered.ts#L1-L1).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let stage = |args: &[&str]| {
        let status = Command::new("git")
            .arg("mesh")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git mesh available");
        assert!(
            status.status.success(),
            "git mesh {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    // Pre-existing mesh that anchors both the code range AND the wiki page —
    // satisfies `MeshIndex::is_covered`'s "same mesh anchors both" predicate.
    stage(&["add", "billing/covered-flow", "src/covered.ts#L1-L1", "wiki/billing.md"]);
    stage(&["why", "billing/covered-flow", "-m", "pre-existing"]);
    stage(&["commit", "billing/covered-flow"]);

    let bin = env!("CARGO_BIN_EXE_wiki");
    let output = Command::new(bin)
        .args(["scaffold", "**/*.md"])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki scaffold");
    assert!(
        output.status.success(),
        "scaffold failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("src/uncovered.ts#L1-L1"),
        "uncovered link must still be emitted; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("src/covered.ts#L1-L1"),
        "covered link must be filtered out of scaffold output; got:\n{stdout}"
    );
}
