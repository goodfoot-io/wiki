//! Integration tests pinning the relocation-evidence amendment's witness
//! matrix (plans/relocation-evidence-amendment.md) as green in-suite
//! controls: second moves after a committed relocation auto-relocate with a
//! clean re-check, verbatim quotes never win the cross-file tier, committed
//! renames resolve via history, and lightly-edited fuzzy relocations report
//! one honest bump-required diagnostic instead of a false "certified content
//! moved" claim.

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

/// Run `wiki check --fix` (plus extra args) from `cwd`.
fn wiki_check_fix(cwd: &Path, extra: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_wiki");
    let mut args = vec!["check", "--fix"];
    args.extend_from_slice(extra);
    args.push("**/*.md");
    Command::new(bin)
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("run wiki check --fix")
}

/// Run `wiki check` (plus extra args) from `cwd`.
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

fn init_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    tmp
}

fn write_certified_page(root: &Path, name: &str, value: &str, body: &str) {
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    let content = format!(
        "---\ntitle: {name}\nsummary: A page about {name}.\nlinks-reviewed: {value}\n---\n\n{body}\n"
    );
    std::fs::write(root.join("wiki").join(name), content).unwrap();
}

/// The exact-tier certified block used by the drift-fix fixtures.
const BLOCK: &str = "fn canonical() {\n    compute()\n    resolve()\n}\n";

/// An emptied target that keeps four lines: the certified range L2-L4 still
/// fits, so a cross-file move reaches the move scan instead of classifying
/// Broken on the extent check.
const EMPTIED: &str = "// emptied\n// emptied\n// emptied\n// emptied\n";

/// The fuzzy-tier certified block: the pair `FUZZY_BLOCK` /
/// `FUZZY_BLOCK_EDITED` is the one the in-suite classification tests prove
/// at-threshold (one whole-line edit, 5 of 6 lines identical).
const FUZZY_BLOCK: &str = "\
fn resolve_target_path(root: &Path, page: &str, href: &str) -> String {
    let joined = root.join(page).join(href);
    let normalized = normalize_segments(&joined);
    assert!(normalized.is_relative(), \"target escaped the repo\");
    normalized
}";

/// `FUZZY_BLOCK` with the third line's call renamed.
const FUZZY_BLOCK_EDITED: &str = "\
fn resolve_target_path(root: &Path, page: &str, href: &str) -> String {
    let joined = root.join(page).join(href);
    let normalized = canonicalize_segments(&joined);
    assert!(normalized.is_relative(), \"target escaped the repo\");
    normalized
}";

/// `FUZZY_BLOCK` with three whole-line changes: the Jaccard score stays
/// below the fuzzy tier's threshold (the move scan cannot follow it), but
/// the similarity keeps git's rename detection on — the committed record is
/// an `R` row, which the deletion-history walk needs.
const FUZZY_BLOCK_EDITED_HEAVY: &str = "\
fn resolve_target_path(root: &Path, page: &str, href: &str) -> String {
    let cfg = Config::load();
    let joined = cfg.resolve(page, href);
    assert!(normalized.is_relative(), \"target escaped the repo\");
    normalized
}";

/// Commit the certified fixture: `[code](../src/target.rs#L2-L4)` covering
/// the block at `src/target.rs` lines 2-4, page field `1`.
fn seed_certified(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/target.rs"), format!("// preamble\n{BLOCK}")).unwrap();
    write_certified_page(root, "page.md", "1", "See [code](../src/target.rs#L2-L4).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify"]);
}

/// stdout + stderr concatenated, for content assertions.
fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

// ── Witness row 1: second move after a committed first relocation ───────────

/// The first relocation is committed (page href + moved block), then the
/// block moves again via a second committed rename. The epoch record still
/// holds the ORIGINAL coordinates, so only the certified-content-keyed move
/// scan can find the block: the second `--fix` auto-relocates, the post-fix
/// re-check is clean, and the page is not rewritten again (no loop).
#[test]
fn second_move_after_committed_relocation_auto_relocates() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // First relocation: a committed rename carries the block to src/moved.rs
    // with a one-line preamble shift (block now at L3-L5).
    git(root, &["mv", "src/target.rs", "src/moved.rs"]);
    std::fs::write(root.join("src/moved.rs"), format!("// preamble\n// x\n{BLOCK}")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "first move"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "first relocation must exit 0; got:\n{}",
        combined(&out)
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(page.contains("../src/moved.rs#L3-L5"), "href relocated: {page}");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "relocate page"]);

    // Second move: the block leaves moved.rs for final.rs via another
    // committed rename, now with a four-line preamble (block at L5-L7).
    git(root, &["mv", "src/moved.rs", "src/final.rs"]);
    std::fs::write(root.join("src/final.rs"), format!("// a\n// b\n// c\n// d\n{BLOCK}")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "second move"]);

    let out2 = wiki_check_fix(root, &[]);
    assert_eq!(
        out2.status.code(),
        Some(0),
        "second relocation must exit 0; got:\n{}",
        combined(&out2)
    );
    let page2 = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(page2.contains("../src/final.rs#L5-L7"), "href relocated again: {page2}");

    // Re-check over the relocated page stays green — no loop.
    let out3 = wiki_check_fix(root, &[]);
    assert_eq!(
        out3.status.code(),
        Some(0),
        "re-check must stay green; got:\n{}",
        combined(&out3)
    );
    let page3 = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert_eq!(page2, page3, "re-check must not rewrite the page again");
}

/// Same-file variant of the second move: after the committed relocation the
/// block shifts down within the relocated file itself, and `--fix` follows
/// with a range-only rewrite that stays green on re-check.
#[test]
fn second_same_file_shift_after_committed_relocation_auto_relocates() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    git(root, &["mv", "src/target.rs", "src/moved.rs"]);
    std::fs::write(root.join("src/moved.rs"), format!("// preamble\n// x\n{BLOCK}")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "first move"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "relocate page"]);

    // The block shifts down four lines within the relocated file.
    std::fs::write(root.join("src/moved.rs"), format!("// a\n// b\n// c\n// d\n{BLOCK}")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "shift block again"]);

    let out2 = wiki_check_fix(root, &[]);
    assert_eq!(
        out2.status.code(),
        Some(0),
        "same-file second move must exit 0; got:\n{}",
        combined(&out2)
    );
    let page2 = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(page2.contains("../src/moved.rs#L5-L7"), "range relocated: {page2}");

    let out3 = wiki_check_fix(root, &[]);
    assert_eq!(
        out3.status.code(),
        Some(0),
        "re-check must stay green; got:\n{}",
        combined(&out3)
    );
    let page3 = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert_eq!(page2, page3, "re-check must not rewrite the page again");
}

// ── Witness row 2: a verbatim quote never wins the cross-file tier ──────────

/// The certified block is quoted byte-identically in an unrelated page while
/// the original target is emptied. The quote carries no identity evidence —
/// no rename row anywhere connects it to the source — so it must never win
/// the cross-file tier: the link classifies Drift with the bump remedy, the
/// href stays put, and `--fix` exits 1 (review required).
#[test]
fn verbatim_quote_in_unrelated_page_never_wins_the_fuzzy_tier() {
    let tmp = init_repo();
    let root = tmp.path();
    seed_certified(root);

    // The certified range L2-L4 still fits the emptied target (Drift, not
    // Broken), and the quoting page holds the block verbatim at L1-L3.
    std::fs::write(root.join("src/target.rs"), EMPTIED).unwrap();
    std::fs::write(root.join("wiki/quote.md"), BLOCK).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "quote the block"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "quote hijack must exit 1 (Drift); got:\n{}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("links-reviewed"),
        "skip remedy must name the field"
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("../src/target.rs#L2-L4"),
        "href must stay put on an un-evidenced quote: {page}"
    );
    assert!(
        !page.contains("quote.md"),
        "href must never point at the quoting page: {page}"
    );
}

// ── Witness row 3: committed renames resolve via history ────────────────────

/// The rename is committed and the destination edited beyond move-scan
/// recognition (three whole-line changes: the Jaccard score stays below the
/// fuzzy threshold, so the link classifies Broken), yet similar enough that
/// git records the rename (`R###` row). The successor comes from git history
/// alone — the deletion-history walk, since `--follow --diff-filter=R` on a
/// deleted path renders the rename's old side as a plain deletion and yields
/// nothing. The range fragment is preserved verbatim, and the re-check
/// honestly flags the edited content for review (exit 1, bump remedy) —
/// nothing is silently passed.
#[test]
fn committed_rename_with_heavy_edits_resolves_successor_via_history() {
    let tmp = init_repo();
    let root = tmp.path();
    // The six-line certified block, anchored in one commit.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/target.rs"), format!("// preamble\n{FUZZY_BLOCK}")).unwrap();
    write_certified_page(root, "page.md", "1", "See [code](../src/target.rs#L2-L7).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify fuzzy block"]);

    git(root, &["mv", "src/target.rs", "src/new.rs"]);
    std::fs::write(
        root.join("src/new.rs"),
        format!("// preamble\n{FUZZY_BLOCK_EDITED_HEAVY}"),
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "rename and rewrite"]);

    let out = wiki_check_fix(root, &[]);
    let text = combined(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "edited content needs review: exit 1; got:\n{text}"
    );
    assert!(
        text.contains("renamed to src/new.rs"),
        "successor must come from committed history: {text}"
    );
    assert!(
        text.contains("links-reviewed"),
        "re-check must demand review of the edited content: {text}"
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("../src/new.rs#L2-L7"),
        "href follows the committed successor, fragment preserved: {page}"
    );
}

// ── Witness row 4: fuzzy relocation reports honestly ────────────────────────

/// A committed rename whose destination is a lightly-edited near-copy of the
/// certified block relocates the href (the fuzzy tier follows the move) but
/// must not claim "certified content moved": the run exits 1 with exactly
/// one coherent bump-required diagnostic, no `fixed:` line, and the re-check
/// over the rewritten page reports the same single diagnostic — never a fix
/// contradicted by the re-check, never a loop.
#[test]
fn lightly_edited_fuzzy_relocation_reports_honest_bump_diagnostic() {
    let tmp = init_repo();
    let root = tmp.path();
    // A fuzzy-fixture target certified in ONE commit: the certified range
    // covers the six-line block, and the anchor is that single commit.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/target.rs"), format!("// preamble\n{FUZZY_BLOCK}")).unwrap();
    write_certified_page(root, "page.md", "1", "See [code](../src/target.rs#L2-L7).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify fuzzy block"]);

    // The block is renamed to src/moved.rs and its third line edited
    // (block now at L3-L8): exact tiers find nothing, the fuzzy tier does.
    git(root, &["mv", "src/target.rs", "src/moved.rs"]);
    std::fs::write(root.join("src/moved.rs"), format!("// preamble\n// x\n{FUZZY_BLOCK_EDITED}"))
        .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "rename and edit the block"]);

    let out = wiki_check_fix(root, &[]);
    let text = combined(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "fuzzy relocation needs re-certification: exit 1; got:\n{text}"
    );
    assert!(
        text.contains("relocated to src/moved.rs lines 3-8"),
        "skip must report the relocation truthfully: {text}"
    );
    assert!(
        text.contains("not byte-identical to the certified block")
            && text.contains("bump `links-reviewed:`"),
        "skip must demand re-certification: {text}"
    );
    assert!(
        !text.contains("fixed:"),
        "no fixed: line may contradict the re-check: {text}"
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("../src/moved.rs#L3-L8"),
        "href must still follow the move: {page}"
    );

    // The re-check over the rewritten page reports the same single
    // diagnostic — coherent, deterministic, no loop.
    let out2 = wiki_check_fix(root, &[]);
    let text2 = combined(&out2);
    assert_eq!(
        out2.status.code(),
        Some(1),
        "re-check must agree with the skip; got:\n{text2}"
    );
    assert!(
        text2.contains("relocated to src/moved.rs lines 3-8"),
        "re-check must report the same diagnostic: {text2}"
    );
    assert!(!text2.contains("fixed:"), "re-check must not fix: {text2}");
    let page2 = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert_eq!(page, page2, "re-check must not rewrite the page again");
}

// ── Witness row: deleted duplicate — content identity resolves the pairing ──

/// Variant A (round-2 witness q41): a duplicate with the same display text
/// is deleted and the survivor is re-pointed to its block's new location
/// (the target shifted down two lines). Content identity resolves the
/// pairing — the locator's content equals a same-label candidate's certified
/// block — so the link is Healthy: the check exits 0 with no diagnostics.
#[test]
fn check_deleted_duplicate_repointed_to_block_passes() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "fn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n",
    )
    .unwrap();
    write_certified_page(
        root,
        "page.md",
        "1",
        "See [canonical](/src/lib.rs#L1-L3) and [canonical](/src/lib.rs#L5-L7).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify two same-display-text links"]);

    // The first link is deleted and the target shifts down two lines; the
    // survivor is re-pointed to follow its block (L5-L7 -> L7-L9).
    std::fs::write(
        root.join("src/lib.rs"),
        "// a\n// b\nfn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n",
    )
    .unwrap();
    write_certified_page(root, "page.md", "1", "See [canonical](/src/lib.rs#L7-L9).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "delete duplicate, re-point survivor"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "content identity must resolve the pairing; got:\n{}",
        combined(&out)
    );
    assert!(
        !combined(&out).contains("could not verify"),
        "no diagnostics expected:\n{}",
        combined(&out)
    );
}

/// Variant B (round-2 witness q41b): a duplicate is deleted and the target
/// shifts down, but the survivor's href is left stale. `--fix` relocates it
/// via the fragment-matched record, and the same run's re-check resolves
/// Healthy via the content-identity carve-out — the run reports `fixed:`
/// with no Unknown diagnostic and exits 0, converging (a second run stays
/// clean).
#[test]
fn check_fix_converges_after_deleted_duplicate_block_shift() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "fn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n",
    )
    .unwrap();
    write_certified_page(
        root,
        "page.md",
        "1",
        "See [canonical](/src/lib.rs#L1-L3) and [canonical](/src/lib.rs#L5-L7).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify two same-display-text links"]);

    // The first link is deleted and the target shifts down two lines; the
    // survivor's href stays stale at the old coordinates.
    std::fs::write(
        root.join("src/lib.rs"),
        "// a\n// b\nfn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n",
    )
    .unwrap();
    write_certified_page(root, "page.md", "1", "See [canonical](/src/lib.rs#L5-L7).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "delete duplicate, href stale"]);

    let out = wiki_check_fix(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the run must converge; got:\n{}",
        combined(&out)
    );
    let text = combined(&out);
    assert!(
        text.contains("fixed:"),
        "the relocation must be applied: {text}"
    );
    assert!(
        text.contains("#L5-L7") && text.contains("#L7-L9"),
        "the fixed: line must name both locators: {text}"
    );
    assert!(
        !text.contains("could not verify"),
        "the re-check must not flag the rewritten link: {text}"
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("/src/lib.rs#L7-L9"),
        "the href must follow the block: {page}"
    );
    // A second run stays clean — no loop.
    let out2 = wiki_check(root, &[]);
    assert_eq!(
        out2.status.code(),
        Some(0),
        "re-check must stay clean; got:\n{}",
        combined(&out2)
    );
}

/// Genuine pairing ambiguity: the duplicate is deleted and the survivor is
/// re-pointed to content that matches NO same-label candidate's certified
/// block. The check reports the pairing-ambiguity message — never the
/// multi-location text — and exits 1; `--fix` skips with the same reason
/// and leaves the page byte-untouched.
#[test]
fn check_deleted_duplicate_genuine_ambiguity_reports_pairing_message() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "fn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n\nfn gamma() {\n    g()\n}\n",
    )
    .unwrap();
    write_certified_page(
        root,
        "page.md",
        "1",
        "See [canonical](/src/lib.rs#L1-L3) and [canonical](/src/lib.rs#L5-L7).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify two same-display-text links"]);

    // The first link is deleted; the survivor is re-pointed to content that
    // matches no candidate's certified block.
    write_certified_page(root, "page.md", "1", "See [canonical](/src/lib.rs#L9-L11).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "delete duplicate, re-point to uncertified content"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "genuine pairing ambiguity must exit 1; got:\n{}",
        combined(&out)
    );
    let text = combined(&out);
    assert!(
        text.contains("a duplicate link with this display text was removed"),
        "message must name the deleted duplicate:\n{text}"
    );
    assert!(
        !text.contains("occurs at multiple locations"),
        "message must not claim a multi-location ambiguity:\n{text}"
    );

    let out_fix = wiki_check_fix(root, &[]);
    assert_eq!(
        out_fix.status.code(),
        Some(1),
        "--fix must also exit 1; got:\n{}",
        combined(&out_fix)
    );
    assert!(
        combined(&out_fix).contains("skipped:"),
        "--fix must skip the link:\n{}",
        combined(&out_fix)
    );
    assert!(
        !combined(&out_fix).contains("fixed:"),
        "--fix must not apply anything:\n{}",
        combined(&out_fix)
    );
    let page = std::fs::read_to_string(root.join("wiki/page.md")).expect("read page");
    assert!(
        page.contains("/src/lib.rs#L9-L11"),
        "the page must be byte-untouched: {page}"
    );
}

/// Control: two same-label links certify byte-identical blocks; one is
/// deleted and the survivor's locator still contains those bytes. The
/// any-candidate carve-out is outcome-invariant for identical candidates —
/// Healthy, exit 0.
#[test]
fn check_deleted_duplicate_identical_blocks_stay_healthy() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "fn alpha() {\n    a()\n}\n\nfn alpha() {\n    a()\n}\n",
    )
    .unwrap();
    write_certified_page(
        root,
        "page.md",
        "1",
        "See [canonical](/src/lib.rs#L1-L3) and [canonical](/src/lib.rs#L5-L7).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify two identical blocks"]);

    // Delete the first link only; the survivor keeps its locator and its
    // certified bytes.
    write_certified_page(root, "page.md", "1", "See [canonical](/src/lib.rs#L5-L7).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "delete duplicate, survivor untouched"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "identical candidates must stay healthy; got:\n{}",
        combined(&out)
    );
}

/// Control: deletion alone with an unmoved survivor locator passes clean —
/// the survivor still matches its epoch record by fragment, so nothing
/// flags. Guards against over-flagging the deleted-duplicate case.
#[test]
fn check_deleted_duplicate_with_unmoved_survivor_passes() {
    let tmp = init_repo();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "fn alpha() {\n    a()\n}\n\nfn beta() {\n    b()\n}\n",
    )
    .unwrap();
    write_certified_page(
        root,
        "page.md",
        "1",
        "See [canonical](/src/lib.rs#L1-L3) and [canonical](/src/lib.rs#L5-L7).",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "certify two same-display-text links"]);

    // Delete the first link only; the survivor keeps its locator and its
    // certified content.
    write_certified_page(root, "page.md", "1", "See [canonical](/src/lib.rs#L5-L7).");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "delete duplicate, survivor untouched"]);

    let out = wiki_check(root, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "unmoved survivor must stay clean; got:\n{}",
        combined(&out)
    );
}
