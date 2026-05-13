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

    // A file without frontmatter is a valid plain markdown page and must NOT
    // appear in the parse-error block.
    std::fs::write(wiki_dir.join("no_frontmatter.md"), "# Just a body.\n").unwrap();

    // Frontmatter without `title:` is valid — not an error.
    std::fs::write(
        wiki_dir.join("missing_title.md"),
        "---\nsummary: x\n---\n\nbody\n",
    )
    .unwrap();

    // Category 1: EmptyTitle — `title:` present but empty.
    std::fs::write(
        wiki_dir.join("empty_title.md"),
        "---\ntitle:\nsummary: x\n---\n\nbody\n",
    )
    .unwrap();

    // Category 2: Unreadable — non-UTF-8 bytes.
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

    // The two bad files must be listed with their exact reason strings.
    assert!(
        stdout.contains("wiki/empty_title.md (frontmatter present but `title:` is empty)"),
        "EmptyTitle reason missing:\n{stdout}"
    );
    assert!(
        stdout.contains("wiki/unreadable.md (file could not be read:"),
        "Unreadable reason missing:\n{stdout}"
    );

    // Files without a frontmatter block, or with frontmatter but no `title:`
    // key, are valid plain markdown pages and must not surface as parse errors.
    let block_end_check = stdout.find("\n---\n").unwrap_or(stdout.len());
    assert!(
        !stdout[..block_end_check].contains("no_frontmatter.md"),
        "no_frontmatter.md must not appear in the parse-error block:\n{stdout}"
    );
    assert!(
        !stdout[..block_end_check].contains("missing_title.md"),
        "missing_title.md must not appear in the parse-error block:\n{stdout}"
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

    // Only files that fail to parse — no clean file with links.
    std::fs::write(
        wiki_dir.join("empty_title.md"),
        "---\ntitle:\n---\n\nbody\n",
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
        !stdout.contains("No internal fragment links with line ranges were found"),
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
        "wiki scaffold --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON output");

    // schemaVersion must be 1.
    assert_eq!(v["schemaVersion"], 1, "schemaVersion must be 1:\n{stdout}");

    // parseErrors must be present and empty.
    assert!(
        v["parseErrors"].is_array(),
        "parseErrors must be array:\n{stdout}"
    );
    assert!(
        v["parseErrors"].as_array().unwrap().is_empty(),
        "parseErrors must be empty:\n{stdout}"
    );

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
    assert_eq!(
        chain.len(),
        1,
        "leading 'Billing' should be trimmed, got chain: {chain:?}"
    );
    assert_eq!(chain[0], "Charge handler");

    // sectionOpening field must be absent — heading chain alone identifies the section.
    assert!(
        mesh.get("sectionOpening").is_none(),
        "sectionOpening field must be absent:\n{stdout}"
    );

    // anchors must be structured objects, with the page section anchor first.
    let anchors = mesh["anchors"].as_array().unwrap();
    assert!(
        anchors.len() >= 2,
        "anchors must contain page section + targets:\n{stdout}"
    );
    assert_eq!(
        anchors[0]["path"], "wiki/billing.md",
        "anchors[0] must be the page section anchor:\n{stdout}"
    );
    assert!(
        anchors[0]["startLine"].is_number(),
        "anchor.startLine must be number:\n{stdout}"
    );
    assert!(
        anchors[0]["endLine"].is_number(),
        "anchor.endLine must be number:\n{stdout}"
    );
    assert_eq!(
        anchors[1]["path"], "src/charge.rs",
        "anchors[1] must be the target:\n{stdout}"
    );

    // Legacy fields must be absent.
    assert!(
        mesh["name"].is_null(),
        "legacy 'name' field must be absent:\n{stdout}"
    );
    assert!(
        mesh["why"].is_null(),
        "legacy 'why' field must be absent:\n{stdout}"
    );
    assert!(
        mesh["wikiFile"].is_null(),
        "legacy 'wikiFile' field must be absent:\n{stdout}"
    );
    assert!(
        mesh["anchor"].is_null(),
        "legacy 'anchor' string field must be absent:\n{stdout}"
    );
}

/// JSON mode with empty corpus still emits the structured object, not `[]`.
#[test]
fn mesh_scaffold_json_empty_corpus_structured_output() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();

    // A valid file with no fragment links → empty corpus.
    std::fs::write(
        wiki_dir.join("page.md"),
        "---\ntitle: Empty\n---\n\nNo links here.\n",
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
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki scaffold --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    // Must be an object (not `[]`).
    assert!(
        v.is_object(),
        "empty corpus must emit object, not array:\n{stdout}"
    );
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

    // A plain markdown file without a frontmatter block — not an error.
    std::fs::write(wiki_dir.join("no_fm.md"), "# No frontmatter\n").unwrap();
    // Frontmatter without `title:` — not an error.
    std::fs::write(
        wiki_dir.join("missing_title.md"),
        "---\nsummary: x\n---\n\nbody\n",
    )
    .unwrap();
    // Category 1: EmptyTitle.
    std::fs::write(
        wiki_dir.join("empty_title.md"),
        "---\ntitle:\n---\n\nbody\n",
    )
    .unwrap();
    // Clean file with no links (so all_inputs.is_empty() → but we want the parse_errors path).
    std::fs::write(
        wiki_dir.join("clean.md"),
        "---\ntitle: Clean\n---\n\nNo links.\n",
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
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary");
    assert!(
        output.status.success(),
        "wiki scaffold --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let errors = v["parseErrors"].as_array().unwrap();
    assert!(
        !errors.is_empty(),
        "parseErrors must be non-empty:\n{stdout}"
    );

    // Check category tags are snake_case.
    let categories: Vec<&str> = errors
        .iter()
        .map(|e| e["category"].as_str().unwrap())
        .collect();
    assert!(
        !categories.contains(&"no_frontmatter"),
        "files without frontmatter must not produce a parse error: {categories:?}"
    );
    assert!(
        !categories.contains(&"missing_title"),
        "frontmatter without `title:` must not produce a parse error: {categories:?}"
    );
    assert!(
        categories.contains(&"empty_title"),
        "empty_title category missing: {categories:?}"
    );

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

    std::fs::write(
        wiki_dir.join("page.md"),
        "---\ntitle: My Page\n---\n\nSee [x](src/x.rs#L1-L2) at the top.\n\n# Heading below\n",
    )
    .unwrap();
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
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let mesh = &v["pages"][0]["meshes"][0];
    let chain = mesh["headingChain"].as_array().unwrap();
    assert!(
        chain.is_empty(),
        "top-of-file link must have empty headingChain, got: {chain:?}"
    );
}

/// JSON mode — a file in `parseErrors[]` must NOT appear in `pages[]` (disjoint
/// schema). A file with malformed frontmatter that also contains fragment links
/// should land in parseErrors and nowhere else.
#[test]
fn mesh_scaffold_json_parse_error_page_not_in_pages() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();

    // File with empty title (EmptyTitle parse error) — should be in parseErrors only.
    std::fs::write(
        wiki_dir.join("bad.md"),
        "---\ntitle:\nsummary: x\n---\n\nSee [x](src/x.rs#L1-L2) for details.\n",
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
        "wiki scaffold --format json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    // bad.md must be in parseErrors.
    let errors = v["parseErrors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["path"].as_str().is_some_and(|p| p.contains("bad.md"))),
        "bad.md must appear in parseErrors:\n{stdout}"
    );

    // bad.md must NOT be in pages.
    let pages = v["pages"].as_array().unwrap();
    assert!(
        !pages
            .iter()
            .any(|p| p["path"].as_str().is_some_and(|pp| pp.contains("bad.md"))),
        "bad.md must not appear in pages[]:\n{stdout}"
    );

    // clean.md must be in pages.
    assert!(
        pages
            .iter()
            .any(|p| p["path"].as_str().is_some_and(|pp| pp.contains("clean.md"))),
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
        e["path"]
            .as_str()
            .is_some_and(|p| p.contains("unreadable.md"))
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
        p["path"]
            .as_str()
            .is_some_and(|pp| pp.contains("unreadable.md"))
    });
    assert!(
        bad_page.is_none(),
        "unreadable.md must not appear in pages[]:\n{stdout}"
    );
}

/// `wiki scaffold` against a wiki file outside the `wiki/` directory must
/// succeed. Content-based discovery finds any `.md` with title+summary
/// frontmatter regardless of path.
#[test]
fn mesh_scaffold_handles_file_outside_wiki_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("floats")).unwrap();
    std::fs::write(
        root.join("wiki/index.md"),
        "---\ntitle: Default Index\nsummary: D.\n---\nHi.\n",
    )
    .unwrap();
    // Float outside wiki/ — has valid frontmatter so it is a wiki member.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/code.rs"), "fn x() {}\n").unwrap();
    std::fs::write(
        root.join("floats/notes.md"),
        "---\ntitle: Notes\nsummary: N.\n---\nSee [code](../src/code.rs#L1-L1).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(
        root,
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
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
        .args(["scaffold", "../floats/notes.md"])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki scaffold");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "scaffold against a file outside wiki/ must succeed; \
         status={:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status
    );
    // Scaffold must emit a `git mesh add` line that anchors the file to its
    // fragment-link target — if the run silently became a no-op the success
    // assertion alone would not catch it.
    assert!(
        stdout.contains("git mesh add") && stdout.contains("src/code.rs#L1-L1"),
        "scaffold must emit a mesh entry covering the file's fragment link; \
         stdout: {stdout}\nstderr: {stderr}"
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
    git(
        root,
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
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

/// `wiki scaffold` treats existing meshes as the anchor for a wiki section:
///
/// - When some mesh M anchors the exact `(page_path, section_start,
///   section_end)` triple of a section, the section's links are routed into M.
/// - A link whose code anchor is already in M is dropped from the emission.
/// - A link whose code anchor is **not** in M becomes
///   `git mesh add M <code-anchor>` — a stage-only command that extends M.
///   The `git mesh why` line is suppressed since M already has a why.
/// - A section with no owning mesh falls through to today's behavior: a new
///   `git mesh add <fresh-slug>` block with a `git mesh why` companion.
///
/// Reproduction layout: one wiki page with three sections, each exercising a
/// different scenario.
#[test]
fn mesh_scaffold_extends_existing_section_mesh_with_new_code_links() {
    if Command::new("git-mesh").arg("--version").output().is_err() {
        eprintln!("skipping: git-mesh not installed");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/fully.ts"), "// fully\n").unwrap();
    std::fs::write(root.join("src/extend.ts"), "// extend\n").unwrap();
    std::fs::write(root.join("src/fresh.ts"), "// fresh\n").unwrap();

    // Section line numbers (1-indexed) after the 4-line frontmatter + blank:
    //   6:  ## Fully covered
    //   7:  (blank)
    //   8:  See [fully](../src/fully.ts#L1-L1).
    //   9:  (blank)
    //  10:  ## Extends existing
    //  11:  (blank)
    //  12:  See [extend](../src/extend.ts#L1-L1).
    //  13:  (blank)
    //  14:  ## Fresh section
    //  15:  (blank)
    //  16:  See [fresh](../src/fresh.ts#L1-L1).
    // Section ranges per scaffold's grouping: [heading .. last content line].
    std::fs::write(
        root.join("wiki/billing.md"),
        "---\ntitle: Billing\nsummary: bill\n---\n\n\
         ## Fully covered\n\n\
         See [fully](../src/fully.ts#L1-L1).\n\n\
         ## Extends existing\n\n\
         See [extend](../src/extend.ts#L1-L1).\n\n\
         ## Fresh section\n\n\
         See [fresh](../src/fresh.ts#L1-L1).\n",
    )
    .unwrap();

    git(root, &["init", "-q", "-b", "main"]);
    git(
        root,
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
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

    // Discover scaffold's section ranges by running it once with no
    // pre-existing meshes — the emitted `wiki/billing.md#L<s>-L<e>` anchors
    // are the section keys we need to stage against.
    let bin = env!("CARGO_BIN_EXE_wiki");
    let baseline = Command::new(bin)
        .args(["scaffold", "**/*.md"])
        .current_dir(root.join("wiki"))
        .output()
        .expect("run wiki scaffold (baseline)");
    let baseline_stdout = String::from_utf8_lossy(&baseline.stdout).into_owned();
    let extract = |label: &str| -> String {
        // Find the `## <label>` block and the next `wiki/billing.md#L..-L..` anchor inside it.
        let block_idx = baseline_stdout
            .find(&format!("## {label}"))
            .expect("section label present in baseline");
        let tail = &baseline_stdout[block_idx..];
        let anchor_start = tail
            .find("wiki/billing.md#L")
            .expect("section anchor present");
        let anchor_tail = &tail[anchor_start..];
        let anchor_end = anchor_tail.find([' ', '\\', '\n']).unwrap();
        anchor_tail[..anchor_end].to_string()
    };
    let fully_anchor = extract("Fully covered");
    let extend_anchor = extract("Extends existing");

    // Pre-existing meshes:
    //   - `billing/fully-covered` owns the section AND the code link → drop draft.
    //   - `billing/extend-target` owns the section but NOT the code link →
    //     emission becomes `git mesh add billing/extend-target src/extend.ts#L1-L1`.
    stage(&[
        "add",
        "billing/fully-covered",
        &fully_anchor,
        "src/fully.ts#L1-L1",
    ]);
    stage(&[
        "why",
        "billing/fully-covered",
        "-m",
        "pre-existing fully covered",
    ]);
    stage(&["commit", "billing/fully-covered"]);

    stage(&["add", "billing/extend-target", &extend_anchor]);
    stage(&[
        "why",
        "billing/extend-target",
        "-m",
        "pre-existing extension target",
    ]);
    stage(&["commit", "billing/extend-target"]);

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

    // 1. Fully covered: code anchor must not appear at all; no block created.
    assert!(
        !stdout.contains("src/fully.ts"),
        "fully-covered section must be filtered out entirely; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("billing/fully-covered"),
        "no block should reference billing/fully-covered; got:\n{stdout}"
    );

    // 2. Extends existing: emit `git mesh add billing/extend-target src/extend.ts#L1-L1`
    //    with no `git mesh why billing/extend-target` line.
    assert!(
        stdout.contains("git mesh add billing/extend-target"),
        "expected extension block targeting existing mesh; got:\n{stdout}"
    );
    assert!(
        stdout.contains("src/extend.ts#L1-L1"),
        "expected new code anchor in extension block; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("git mesh why billing/extend-target"),
        "extension blocks must not invite a why rewrite; got:\n{stdout}"
    );
    // The section's wiki anchor itself is dropped from the extension emission
    // — git-mesh already carries it.
    assert!(
        !stdout.contains(&format!(
            "git mesh add billing/extend-target \\\n  {extend_anchor}"
        )),
        "extension block must not re-add the section's wiki anchor; got:\n{stdout}"
    );

    // 3. Fresh section: today's behavior — new slug, with a `git mesh why` line.
    assert!(
        stdout.contains("git mesh add wiki/fresh-section"),
        "expected new mesh for the fresh section; got:\n{stdout}"
    );
    assert!(
        stdout.contains("git mesh why wiki/fresh-section"),
        "new-mesh blocks still need a why line; got:\n{stdout}"
    );
}

/// When a fragment link references a source path that does not exist on disk,
/// the mesh is dropped from scaffold output and an advisory line is emitted.
#[test]
fn mesh_scaffold_drops_mesh_with_missing_anchor_path() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();

    // Page links to src/present.rs (exists) and src/missing.rs (does not exist).
    std::fs::write(
        wiki_dir.join("page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\n\
         ## Section A\n\nSee [present](../src/present.rs#L1-L5).\n\n\
         ## Section B\n\nSee [missing](../src/missing.rs#L1-L5).\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("present.rs"), "// present\n").unwrap();
    // src/missing.rs intentionally NOT created.

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

    // Section A's mesh (present.rs exists) must appear.
    assert!(
        stdout.contains("src/present.rs#L1-L5"),
        "mesh for present anchor must appear:\n{stdout}"
    );

    // Section B's mesh (missing.rs absent) must be dropped — the advisory names it
    // but no `git mesh add` block for section-b must appear.
    assert!(
        !stdout.contains("git mesh add wiki/section-b"),
        "git mesh add block for missing-anchor mesh must be absent:\n{stdout}"
    );

    // An advisory line must name the skipped mesh and path.
    assert!(
        stdout.contains("Skipped mesh") && stdout.contains("src/missing.rs"),
        "advisory line for dropped mesh must appear:\n{stdout}"
    );
}

/// JSON mode: dropped meshes appear in the top-level `droppedMeshes` array.
#[test]
fn mesh_scaffold_json_dropped_meshes_array() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();

    std::fs::write(
        wiki_dir.join("page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [missing](../src/missing.rs#L1-L5).\n",
    )
    .unwrap();
    // src/missing.rs intentionally absent.

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
        "wiki scaffold --format json failed: stderr=\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");

    let dropped = v["droppedMeshes"].as_array().expect("droppedMeshes array");
    assert!(
        !dropped.is_empty(),
        "droppedMeshes must be non-empty:\n{stdout}"
    );

    let entry = &dropped[0];
    assert!(entry["slug"].is_string(), "slug must be string:\n{stdout}");
    assert_eq!(
        entry["missingPath"].as_str().unwrap_or(""),
        "src/missing.rs",
        "missingPath must be the missing file:\n{stdout}"
    );
    assert!(entry["page"].is_string(), "page must be string:\n{stdout}");
}

/// The missing-path check must honor `--source`: under `--source=head` an anchor
/// whose target was deleted from the worktree but still lives in HEAD must NOT
/// be dropped, while under the default `--source=worktree` (or omitted) the same
/// scaffold run must drop it.
#[test]
fn mesh_scaffold_missing_path_check_respects_source_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let wiki_dir = tmp.path().join("wiki");
    std::fs::create_dir_all(&wiki_dir).unwrap();

    std::fs::write(
        wiki_dir.join("page.md"),
        "---\ntitle: Page\nsummary: A page.\n---\n\nSee [target](../src/target.rs#L1-L5).\n",
    )
    .unwrap();
    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("target.rs"), "// target\n").unwrap();

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

    // Delete target.rs from the worktree only — HEAD still has it.
    std::fs::remove_file(src_dir.join("target.rs")).unwrap();

    let bin = env!("CARGO_BIN_EXE_wiki");

    // Default (worktree) source: target.rs is missing → mesh dropped.
    let worktree_out = Command::new(bin)
        .args(["scaffold", "**/*.md", "--format", "json"])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary (worktree)");
    let worktree_stdout = String::from_utf8_lossy(&worktree_out.stdout);
    let worktree_v: serde_json::Value =
        serde_json::from_str(&worktree_stdout).expect("valid JSON (worktree)");
    let worktree_dropped = worktree_v["droppedMeshes"]
        .as_array()
        .expect("droppedMeshes array (worktree)");
    assert_eq!(
        worktree_dropped.len(),
        1,
        "worktree mode must drop the mesh:\n{worktree_stdout}"
    );
    assert_eq!(
        worktree_dropped[0]["missingPath"].as_str().unwrap_or(""),
        "src/target.rs"
    );

    // --source=head: target.rs still exists in HEAD → mesh kept.
    let head_out = Command::new(bin)
        .args([
            "--source",
            "head",
            "scaffold",
            "**/*.md",
            "--format",
            "json",
        ])
        .current_dir(&wiki_dir)
        .output()
        .expect("run wiki binary (head)");
    assert!(
        head_out.status.success(),
        "head mode must exit 0: stderr=\n{}",
        String::from_utf8_lossy(&head_out.stderr)
    );
    let head_stdout = String::from_utf8_lossy(&head_out.stdout);
    let head_v: serde_json::Value =
        serde_json::from_str(&head_stdout).expect("valid JSON (head)");
    let head_dropped = head_v["droppedMeshes"]
        .as_array()
        .expect("droppedMeshes array (head)");
    assert!(
        head_dropped.is_empty(),
        "head mode must NOT drop the mesh (target still in HEAD):\n{head_stdout}"
    );
    let head_pages = head_v["pages"].as_array().expect("pages array (head)");
    assert!(
        !head_pages.is_empty(),
        "head mode must emit the surviving mesh under pages:\n{head_stdout}"
    );
}
