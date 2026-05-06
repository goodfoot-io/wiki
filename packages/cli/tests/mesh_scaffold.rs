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
