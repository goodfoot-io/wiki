use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::frontmatter::{Frontmatter, build_index, parse_frontmatter};
use crate::git::GitReader;
use crate::git::resolve_ref;
use crate::headings::{extract_headings, resolve_heading, Heading};
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links};

use super::check_fix;
use super::drift;

/// Per-run content cache that avoids re-reading the same file from disk.
///
/// Each in-scope wiki page is read at least twice during `collect_for_files`
/// (once for frontmatter, once for link parsing), and once more by the drift
/// pass.  In `--fix` mode `collect_for_files` runs twice and
/// `run_fix_pass` re-reads each file.  This cache collapses redundant reads
/// into a single read per unique path per run.
pub(crate) struct ContentCache {
    cache: HashMap<PathBuf, String>,
}

impl ContentCache {
    pub(crate) fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Return the cached content for `path`, calling `read_fn` on a miss and
    /// storing the result.
    pub(crate) fn get_or_try_read<F, E>(
        &mut self,
        path: &Path,
        read_fn: F,
    ) -> std::result::Result<&str, E>
    where
        F: FnOnce() -> std::result::Result<String, E>,
        E: std::fmt::Display,
    {
        match self.cache.entry(path.to_path_buf()) {
            Entry::Occupied(o) => Ok(o.into_mut().as_str()),
            Entry::Vacant(v) => {
                let content = read_fn()?;
                Ok(v.insert(content).as_str())
            }
        }
    }

    /// Pre-read the given working-tree paths in parallel and populate the cache.
    /// Only successful reads are inserted; failures are left absent so the serial
    /// read path still runs read_fn and produces the exact error diagnostic.
    /// Already-cached paths and duplicates are skipped. No-op for non-WorkingTree
    /// sources (their reads go through a non-thread-safe git handle).
    pub(crate) fn warm_working_tree(&mut self, paths: &[PathBuf]) {
        // Collect the unique, not-yet-cached paths.
        let mut targets: Vec<PathBuf> = Vec::with_capacity(paths.len());
        let mut seen: std::collections::HashSet<&PathBuf> = std::collections::HashSet::new();
        for path in paths {
            if self.cache.contains_key(path) {
                continue;
            }
            if seen.insert(path) {
                targets.push(path.clone());
            }
        }
        if targets.is_empty() {
            return;
        }

        // Bounded worker count: at most one thread per path, capped at 8, and
        // never more than the available parallelism.
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let worker_count = parallelism.min(8).min(targets.len()).max(1);

        // Chunk the targets into contiguous slices, one per worker.
        let chunk_size = targets.len().div_ceil(worker_count);
        let mut collected: Vec<(PathBuf, String)> = Vec::with_capacity(targets.len());
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for chunk in targets.chunks(chunk_size) {
                handles.push(scope.spawn(move || {
                    let mut out: Vec<(PathBuf, String)> = Vec::with_capacity(chunk.len());
                    for path in chunk {
                        if let Ok(content) = std::fs::read_to_string(path) {
                            out.push((path.clone(), content));
                        }
                    }
                    out
                }));
            }
            for handle in handles {
                if let Ok(out) = handle.join() {
                    collected.extend(out);
                }
            }
        });

        for (path, content) in collected {
            self.cache.entry(path).or_insert(content);
        }
    }
}

/// Return `true` when `rel_path` exists as a directory in `source`.
///
/// For `WorkingTree` this delegates to `std::path::Path::is_dir`.  For
/// `Index`/`Head` it checks whether any tracked path has the directory as a
/// prefix, because git does not store directory entries as blobs.
fn source_aware_is_dir(
    repo_root: &Path,
    source: DocSource,
    rel_path: &Path,
    git_reader: Option<&GitReader>,
) -> bool {
    match source {
        DocSource::WorkingTree => repo_root.join(rel_path).is_dir(),
        DocSource::Index | DocSource::Head => {
            let prefix = format!("{}/", rel_path.to_string_lossy().trim_end_matches('/'));
            let paths = if let Some(gr) = git_reader {
                match gr.list_paths(source) {
                    Ok(p) => p,
                    Err(_) => return false,
                }
            } else {
                match source.list_paths(repo_root) {
                    Ok(p) => p,
                    Err(_) => return false,
                }
            };
            paths.iter().any(|p| p.starts_with(&prefix))
        }
    }
}

/// Return `true` when `rel_path` exists in `source`.
///
/// For `WorkingTree` this delegates to `std::path::Path::exists`.  For
/// `Index`/`Head` it checks the git blob store via the index or HEAD tree.
fn source_aware_exists(
    repo_root: &Path,
    source: DocSource,
    rel_path: &str,
    git_reader: Option<&GitReader>,
) -> bool {
    match source {
        DocSource::WorkingTree => repo_root.join(rel_path).exists(),
        DocSource::Index | DocSource::Head => {
            if let Some(gr) = git_reader {
                gr.has_entry(source, rel_path).unwrap_or(false)
            } else {
                match source {
                    DocSource::Index => {
                        crate::git::has_index_entry(repo_root, rel_path).unwrap_or(false)
                    }
                    DocSource::Head => {
                        crate::git::has_head_entry(repo_root, rel_path).unwrap_or(false)
                    }
                    _ => false,
                }
            }
        }
    }
}

/// Read `path` from the chosen `DocSource`.
fn read_via_source(
    path: &Path,
    repo_root: &Path,
    source: DocSource,
    git_reader: Option<&GitReader>,
) -> std::io::Result<String> {
    match source {
        DocSource::WorkingTree => std::fs::read_to_string(path),
        DocSource::Index | DocSource::Head => {
            let path_rel = path
                .strip_prefix(repo_root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            let result = if let Some(gr) = git_reader {
                gr.read_blob(source, &path_rel)
            } else {
                source.read(repo_root, &path_rel)
            };
            match result {
                Ok(Some(s)) => Ok(s),
                Ok(None) => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{path_rel} not present in source {:?}", source),
                )),
                Err(e) => Err(std::io::Error::other(e.to_string())),
            }
        }
    }
}

/// Filter `files` to the candidates the chosen `DocSource` considers present.
fn filter_files_for_source(
    files: Vec<PathBuf>,
    repo_root: &Path,
    source: DocSource,
    git_reader: Option<&GitReader>,
) -> Result<Vec<PathBuf>> {
    if matches!(source, DocSource::WorkingTree) {
        return Ok(files);
    }
    let listed: std::collections::HashSet<String> = if let Some(gr) = git_reader {
        gr.list_paths(source)?.into_iter().collect()
    } else {
        source.list_paths(repo_root)?.into_iter().collect()
    };
    Ok(files
        .into_iter()
        .filter(|p| {
            let rel = p
                .strip_prefix(repo_root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.to_string_lossy().into_owned());
            listed.contains(&rel)
        })
        .collect())
}

// ── Diagnostic types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CheckDiagnostic {
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// Convert a snake_case diagnostic kind to Title Case.
fn kind_title_case(kind: &str) -> String {
    kind.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render one diagnostic block in the human-readable hook format, without any
/// trailing separator. The `---` separator is inserted only *between* blocks by
/// [`render_diagnostics`], never after the last one.
fn format_diagnostic(kind: &str, file: &str, line: usize, message: &str) -> String {
    let mut out = format!("Error: {}\n", kind_title_case(kind));
    if !file.is_empty() {
        out.push_str(&format!("- {file}:{line}\n"));
    }
    out.push_str(&format!("- {message}\n"));
    out
}

/// Render a list of diagnostics, joining blocks with a `\n---\n` separator so it
/// appears only between errors. Returns an empty string when there are none.
fn render_diagnostics(diagnostics: &[CheckDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| format_diagnostic(&d.kind, &d.file, d.line, &d.message))
        .collect::<Vec<_>>()
        .join("\n---\n\n")
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Run the check command.
///
/// Returns the exit code: 0 = valid, 1 = validation errors, 2 = runtime error.
#[allow(clippy::too_many_arguments)]
pub fn run(
    globs: &[String],
    json: bool,
    scan_root: &Path,
    repo_root: &Path,
    no_exit_code: bool,
    source: DocSource,
    fix: bool,
    fix_dry_run: bool,
    print_applied: bool,
) -> Result<i32> {
    // Every hard-error arm funnels through this: print the error (a JSON
    // envelope on stderr under `--format json`, `error: ...` otherwise),
    // then exit 2 — unless `--no-exit-code` is set, in which case the
    // diagnostic still prints but the run reports success. The flag's
    // contract is "report but never fail"; a hard error is reportable the
    // same way a validation error is.
    let hard_exit = |err: &dyn std::fmt::Display| -> i32 {
        if json {
            eprintln!("{}", serde_json::json!({"error": err.to_string()}));
        } else {
            eprintln!("error: {err}");
        }
        if no_exit_code { 0 } else { 2 }
    };

    // Files to check are selected from `scan_root` (the current working
    // directory); globs resolve relative to it.
    //
    // In `--fix` mode an empty discovered set is NOT an error: the user may
    // have deleted the last wiki page (or demoted it to plain markdown), and
    // the fix pass is a no-op over an empty corpus. In non-fix mode we keep
    // the existing exit-2 "no wiki pages found" signal — it is a useful
    // diagnostic. discover_files returns Ok(vec![]) for an empty corpus (no
    // wiki pages found) and Err only for genuine infrastructure failures.
    // Non-fix mode treats an empty corpus as a fatal diagnostic (exit 2); fix
    // mode degrades gracefully over the empty set.
    // The resolution/collision corpus is the entire wiki, scanned from the
    // repo root, so a subtree check resolves titles and detects collisions
    // against every page — making a subdirectory run equivalent to the same
    // pages selected by glob from the repo root.
    //
    // This whole-repo walk happens FIRST and exactly once. In the no-glob
    // case the scoped `files` set is a prefix-filtered subset of it, so we
    // derive it in memory rather than walking the tree a second time.
    // discover_files returns Ok(vec![]) for an empty corpus, which is fine in
    // both modes; real infrastructure failures propagate as Err and fail
    // closed.

    // Open the git repository once when reading from Index or Head so that
    // the same `gix::Repository` handle is reused for all blob reads, path
    // listings, and existence checks in a single run.
    let git_reader = match source {
        DocSource::Index | DocSource::Head => {
            match GitReader::open(repo_root) {
                Ok(gr) => Some(gr),
                Err(e) => return Ok(hard_exit(&e)),
            }
        }
        DocSource::WorkingTree => None,
    };

    let index_files = match discover_files(&[], repo_root, repo_root, source, git_reader.as_ref()) {
        Ok(f) => match filter_files_for_source(f, repo_root, source, git_reader.as_ref()) {
            Ok(f) => f,
            Err(e) => return Ok(hard_exit(&e)),
        },
        Err(e) => {
            // Whole-repo discovery failure: report through the same gate as
            // every other hard error. Without `--no-exit-code` (CI, plain
            // check) it stays fail-closed at exit 2; with the flag (the
            // hook's best-effort contract) it reports and exits 0.
            return Ok(hard_exit(&e));
        }
    };

    // Scoped selection (the pages to validate). When no explicit globs are
    // given, the scoped set is the whole-repo corpus restricted to the
    // `scan_root` subtree, so derive it by in-memory prefix filtering instead
    // of walking the tree again. With explicit globs we MUST keep the dedicated
    // walk: glob selection deliberately includes fixture pages
    // (`is_fixture_path`) that the membership-filtered whole-repo walk omits.
    let files = if globs.is_empty() {
        let prefix = crate::commands::scan_prefix(scan_root, repo_root);
        index_files
            .iter()
            .filter(|p| {
                let rel = p
                    .strip_prefix(repo_root)
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| p.to_string_lossy().into_owned());
                crate::commands::path_under_prefix(&rel, prefix.as_deref())
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        let raw = match discover_files(globs, scan_root, repo_root, source, git_reader.as_ref()) {
            Ok(f) => f,
            Err(e) => return Ok(hard_exit(&e)),
        };
        match filter_files_for_source(raw, repo_root, source, git_reader.as_ref()) {
            Ok(f) => f,
            Err(e) => return Ok(hard_exit(&e)),
        }
    };

    // Empty-corpus semantics: in non-fix mode an empty scoped selection is the
    // "no wiki pages found" fatal diagnostic (exit 2). In `--fix` mode it
    // degrades gracefully: the fix pass is a no-op over an empty set.
    //
    // Decision: the empty corpus is the same class as the other hard errors,
    // so `--no-exit-code` gates it the same way — the message still prints,
    // but the run reports success. The flag's contract is "report but never
    // fail", and a script that wants a nonzero "nothing matched" signal can
    // simply not pass the flag.
    if files.is_empty() && !fix {
        let msg = "no wiki pages found (no .md files matched)";
        return Ok(hard_exit(&msg));
    }

    // Per-run content cache: avoids re-reading wiki pages (frontmatter loop,
    // link loop, and fix passes all read the same files). In fix mode the
    // same cache instance is shared across the pre-check, the fix pass, and
    // the post-fix re-check.
    let mut content_cache = ContentCache::new();

    let diagnostics = match collect_for_files(
        &files,
        &index_files,
        repo_root,
        source,
        git_reader.as_ref(),
        &mut content_cache,
        !fix,
    ) {
        Ok(d) => d,
        Err(e) => return Ok(hard_exit(&e)),
    };

    // ── Fix pass (only in --fix mode) ────────────────────────────────────────
    if fix {
        let plan = match check_fix::run_fix_pass(
            &files,
            repo_root,
            source,
            fix_dry_run,
            &mut content_cache,
        ) {
            Ok(p) => p,
            Err(e) => return Ok(hard_exit(&e)),
        };

        if fix_dry_run {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "fixes": plan.fixes,
                        "skipped": plan.skipped,
                        "unverified": plan.unverified,
                        "certificationSkips": plan.certification_skips,
                        "errors": diagnostics,
                    }))
                    .unwrap()
                );
            } else if plan.fixes.is_empty() && plan.skipped.is_empty() {
                println!("no fixes to apply");
            } else {
                for f in &plan.fixes {
                    println!(
                        "fix: {} line {}: {} -> {}",
                        f.file, f.line, f.old_href, f.new_href
                    );
                }
                for s in &plan.skipped {
                    println!("skip: {} line {}: {}", s.file, s.line, s.reason);
                }
            }
            if (!diagnostics.is_empty()
                || plan.unverified > 0
                || plan.certification_skips > 0)
                && !no_exit_code
            {
                return Ok(1);
            }
            return Ok(0);
        }

        if print_applied {
            for path in &plan.applied_paths {
                println!("{path}");
            }
        }

        if !json {
            for f in &plan.fixes {
                let line = format!(
                    "fixed: {}:{}  broken_link  {} → {}  ({})",
                    f.file, f.line, f.old_href, f.new_href, f.reason
                );
                if print_applied {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            }
            for s in &plan.skipped {
                let line = format!(
                    "skipped: {}:{}  broken_link  reason: {}",
                    s.file, s.line, s.reason
                );
                if print_applied {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            }
        }

        content_cache = ContentCache::new();
        let post_diagnostics = match collect_for_files(
            &files,
            &index_files,
            repo_root,
            source,
            git_reader.as_ref(),
            &mut content_cache,
            // Include the drift pass: the pending-bump rule (current field
            // value differs from the newest committed value) keeps the
            // just-applied relocations and field initializations green.
            true,
        ) {
            Ok(d) => d,
            Err(e) => return Ok(hard_exit(&e)),
        };

        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "fixes": plan.fixes,
                    "skipped": plan.skipped,
                    "appliedPaths": plan.applied_paths,
                    "unverified": plan.unverified,
                    "certificationSkips": plan.certification_skips,
                    "errors": post_diagnostics,
                }))
                .unwrap()
            );
        } else {
            let rendered = render_diagnostics(&post_diagnostics);
            if print_applied {
                eprint!("{rendered}");
            } else {
                print!("{rendered}");
            }
        }

        if (!post_diagnostics.is_empty()
            || plan.unverified > 0
            || plan.certification_skips > 0)
            && !no_exit_code
        {
            return Ok(1);
        }
        return Ok(0);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "errors": diagnostics })).unwrap()
        );
    } else {
        print!("{}", render_diagnostics(&diagnostics));
    }

    if !diagnostics.is_empty() && !no_exit_code {
        Ok(1)
    } else {
        Ok(0)
    }
}

/// Collect diagnostics for the given glob patterns without printing output.
#[allow(dead_code)]
pub fn collect(globs: &[String], repo_root: &Path) -> Result<Vec<CheckDiagnostic>> {
    collect_with_source(globs, repo_root, DocSource::WorkingTree)
}

/// Collect diagnostics with an explicit `DocSource`.
pub fn collect_with_source(
    globs: &[String],
    repo_root: &Path,
    source: DocSource,
) -> Result<Vec<CheckDiagnostic>> {
    // This entry point (hook/tests) scans from the repo root rather than a
    // narrower working directory.  discover_files returns Ok(vec![]) for an
    // empty corpus; propagate that as an error so the caller sees "no wiki
    // pages found" rather than an empty diagnostic list with exit 0.

    // Open the git repository once when reading from Index or Head.
    let git_reader = match source {
        DocSource::Index | DocSource::Head => Some(GitReader::open(repo_root)?),
        DocSource::WorkingTree => None,
    };

    let files = discover_files(globs, repo_root, repo_root, source, git_reader.as_ref())?;
    if files.is_empty() {
        return Err(miette::miette!(
            "no wiki pages found (no .md files matched)"
        ));
    }
    let files = filter_files_for_source(files, repo_root, source, git_reader.as_ref())?;
    let index_files = if globs.is_empty() {
        files.clone()
    } else {
        let raw = discover_files(&[], repo_root, repo_root, source, git_reader.as_ref())?;
        filter_files_for_source(raw, repo_root, source, git_reader.as_ref())?
    };
    let mut content_cache = ContentCache::new();
    collect_for_files(
        &files,
        &index_files,
        repo_root,
        source,
        git_reader.as_ref(),
        &mut content_cache,
        true,
    )
}

/// Extract the anchor portion (after `#`) from a markdown link href, if present.
///
/// Returns `None` when the href contains no `#`. Line-range anchors like
/// `L10-L20` are still returned here; callers must check whether the
/// `FragmentLink::start_line` is `Some` to distinguish line ranges from
/// heading slugs.
fn anchor_of(href: &str) -> Option<&str> {
    href.find('#').map(|i| &href[i + 1..])
}

/// Percent-decode a URL-encoded string.
///
/// Replaces `%XX` hex-encoded bytes with the corresponding byte value and
/// interprets the result as UTF-8. Returns the original string on decode or
/// UTF-8 failure — a partially-decodable href keeps its literal form.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut result: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_val(bytes[i + 1]);
            let lo = hex_val(bytes[i + 2]);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                result.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(result).unwrap_or_else(|_| input.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Cached content and parsed data for a single link target file.
struct CachedTarget {
    headings: Vec<Heading>,
}

/// The drift pass (plan Decision 7): classify every line-range link on each
/// in-scope page against the page's anchor epoch and emit one diagnostic per
/// non-`Healthy`, non-`Moved` outcome. `Moved` is silent in read-only mode —
/// the content is intact, only coordinates drifted, and `--fix` relocates it.
/// A page with range links but no epoch anywhere is `anchor_epoch_missing`
/// (fail-closed); epoch-resolution failures (shallow clone, git errors)
/// propagate as hard errors (exit 2).
fn collect_drift_diagnostics(
    files: &[PathBuf],
    repo_root: &Path,
    source: DocSource,
    git_reader: Option<&GitReader>,
    content_cache: &mut ContentCache,
) -> Result<Vec<CheckDiagnostic>> {
    let mut out = Vec::new();
    // Shared across pages: the move scan's candidate inventory is loaded once
    // per run, on the first link that needs it.
    let mut ctx = drift::MoveScanCtx::new();
    for path in files {
        let content = match content_cache
            .get_or_try_read(path, || read_via_source(path, repo_root, source, git_reader))
        {
            Ok(c) => c,
            Err(_) => continue, // already reported by the main pass
        };
        if !drift::has_line_range_links(content) {
            continue;
        }
        // The engine speaks repo-relative paths; `discover_files` hands us
        // repo-root-joined (absolute) ones.
        let page_path = path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let current_value = drift::read_links_reviewed(content);
        // The newest committed value is the page blob at HEAD; a page absent
        // at HEAD (new file) has none. A HEAD read failure is treated as an
        // absent field, matching the pre-tri-state behavior.
        let committed_value = match read_via_source(path, repo_root, DocSource::Head, git_reader) {
            Ok(head_content) => drift::read_links_reviewed(&head_content),
            Err(_) => drift::LinksReviewedRead::Readable(None),
        };
        let epoch = drift::find_anchor_commit(
            repo_root,
            &page_path,
            &current_value,
            &committed_value,
        )
        .map_err(|e| miette::miette!("{e}"))?;
        if matches!(&epoch, drift::LinkEpoch::Missing) {
            out.push(CheckDiagnostic {
                kind: "anchor_epoch_missing".into(),
                file: page_path.clone(),
                line: 1,
                message: "page has line-range links but no `links-reviewed:` frontmatter \
                          value — run `wiki check --fix` to initialize it, or add the \
                          field by hand"
                    .into(),
            });
            continue;
        }
        let classes = drift::classify_page(repo_root, source, &page_path, content, &epoch, &mut ctx)
            .map_err(|e| miette::miette!("{e}"))?;
        for c in classes {
            let (kind, message) = match &c.outcome {
                drift::DriftOutcome::Healthy | drift::DriftOutcome::Moved { .. } => continue,
                drift::DriftOutcome::Uncertified => (
                    "link_uncertified",
                    format!(
                        "line-range link `{}` was not present at the page's anchor epoch — \
                         bump `links-reviewed:` after reviewing it",
                        c.original_href
                    ),
                ),
                drift::DriftOutcome::Drift => (
                    "link_drift",
                    format!(
                        "content at `{}#L{}-L{}` changed since the anchor epoch — bump \
                         `links-reviewed:` after reviewing it",
                        c.target_path, c.start_line, c.end_line
                    ),
                ),
                drift::DriftOutcome::Broken => (
                    "link_broken",
                    format!(
                        "line-range link `{}` is broken: target `{}` is missing or the \
                         line range no longer fits its current line count",
                        c.original_href, c.target_path
                    ),
                ),
                drift::DriftOutcome::Unknown => (
                    "link_unverified",
                    format!(
                        "could not verify line-range link `{}`: the certified content \
                         occurs at multiple locations — re-anchor the link and bump \
                         `links-reviewed:`",
                        c.original_href
                    ),
                ),
                drift::DriftOutcome::UnknownLabelDeleted => (
                    "link_unverified",
                    format!(
                        "could not verify line-range link `{}`: a duplicate link with \
                         this display text was removed since the last review; the \
                         surviving link cannot be matched to a reviewed record — re-point \
                         it to the block's current location and bump `links-reviewed:`",
                        c.original_href
                    ),
                ),
            };
            out.push(CheckDiagnostic {
                kind: kind.into(),
                file: page_path.clone(),
                line: c.source_line,
                message,
            });
        }
    }
    Ok(out)
}

fn collect_for_files(
    files: &[PathBuf],
    index_files: &[PathBuf],
    repo_root: &Path,
    source: DocSource,
    git_reader: Option<&GitReader>,
    content_cache: &mut ContentCache,
    drift_pass: bool,
) -> Result<Vec<CheckDiagnostic>> {
    let wiki_ignore =
        crate::wikiignore::WikiIgnore::load(repo_root).map_err(|e| miette::miette!("{e}"))?;
    let mut diagnostics: Vec<CheckDiagnostic> = Vec::new();

    if matches!(source, DocSource::WorkingTree) {
        // Pre-warm the cache with parallel reads of the union of pages we are
        // about to read serially. Overlaps blocked reads on hostile filesystems.
        let mut union: Vec<PathBuf> = Vec::with_capacity(index_files.len() + files.len());
        union.extend(index_files.iter().cloned());
        union.extend(files.iter().cloned());
        content_cache.warm_working_tree(&union);
    }

    let files_set: std::collections::HashSet<&PathBuf> = files.iter().collect();

    // ── Parse frontmatter for all pages ──────────────────────────────────────
    let mut pages: Vec<(PathBuf, Frontmatter)> = Vec::new();

    // Iterate the union of index_files (collision context) and files (scoped
    // diagnostics). Files that are only in index_files contribute to the
    // collision pool without producing diagnostics. Files only in files
    // (glob-selected pages that bypass membership filtering) get diagnostics
    // and, if valid, join the collision pool. Avoid double-parsing paths that
    // are in both sets.
    let mut seen: std::collections::HashSet<&PathBuf> = std::collections::HashSet::new();
    for path in index_files.iter().chain(files.iter()) {
        if !seen.insert(path) {
            continue;
        }
        let in_scope = files_set.contains(path);
        let content = match content_cache
            .get_or_try_read(path, || read_via_source(path, repo_root, source, git_reader))
        {
            Ok(c) => c,
            Err(e) => {
                if in_scope {
                    diagnostics.push(CheckDiagnostic {
                        kind: "runtime".into(),
                        file: path.display().to_string(),
                        line: 0,
                        message: format!("Could not read file: {e}"),
                    });
                }
                continue;
            }
        };

        match parse_frontmatter(content, path) {
            Ok(Some(fm)) => {
                pages.push((path.clone(), fm));
            }
            Ok(None) => {
                if in_scope {
                    diagnostics.push(CheckDiagnostic {
                        kind: "frontmatter".into(),
                        file: path.display().to_string(),
                        line: 1,
                        message:
                            "Add a `---` frontmatter block (`title` and `summary` are required), or add this path to `.wiki/.wikiignore` if it isn't a wiki page."
                                .into(),
                    });
                }
            }
            Err(e) => {
                if in_scope {
                    diagnostics.push(CheckDiagnostic {
                        kind: "frontmatter".into(),
                        file: path.display().to_string(),
                        line: 1,
                        message: e.to_string(),
                    });
                }
            }
        }
    }

    // ── Title/alias collisions ────────────────────────────────────────────────
    let (_index, collisions) = build_index(&pages);
    for col in &collisions {
        if files_set.contains(&col.offending_path) {
            diagnostics.push(CheckDiagnostic {
                kind: "collision".into(),
                file: col.offending_path.display().to_string(),
                line: 1,
                message: format!(
                    "Title or alias `{}` is already defined in `{}`. Rename this page's title or remove the conflicting alias.",
                    col.key,
                    col.existing_path.display()
                ),
            });
        }
    }

    // ── Validate links in all in-scope files ─────────────────────────────────
    let mut target_cache: HashMap<PathBuf, CachedTarget> = HashMap::new();
    for path in files {
        let content = match content_cache
            .get_or_try_read(path, || read_via_source(path, repo_root, source, git_reader))
        {
            Ok(c) => c,
            Err(_) => continue,
        };

        let frag_links = parse_fragment_links(content);
        for link in &frag_links {
            if link.kind == LinkKind::External {
                continue;
            }
            // Skip non-file URI schemes that snuck past kind detection (mailto, etc.).
            if link.original_href.starts_with("mailto:") {
                continue;
            }
            // Line-range links are the drift pass's sole authority (plan
            // Decision 7): it classifies them against the page's anchor
            // epoch — including its own target resolution and suffix
            // salvage — so the generic pass must not report them.
            if link.start_line.is_some() {
                continue;
            }

            let decoded_path = percent_decode(&link.path);
            let resolved = crate::commands::resolve_link_path(&decoded_path, path, repo_root);
            let abs = repo_root.join(&resolved);

            // A link whose target is wikiignored is exempt from broken-link and
            // broken-anchor diagnostics: the target is invisible at every wiki
            // surface by design, so the link is valid by definition.
            if wiki_ignore.is_ignored(&resolved) {
                continue;
            }

            // Try to read the target. Directories are valid link targets.
            // Cache target content + computed data to avoid re-reading and
            // re-parsing the same file when multiple links reference it.
            let cached = match target_cache.entry(abs.clone()) {
                Entry::Occupied(o) => Some(o.into_mut()),
                Entry::Vacant(v) => {
                    match content_cache
                        .get_or_try_read(&abs, || read_via_source(&abs, repo_root, source, git_reader))
                    {
                        Ok(c) => {
                            let headings = extract_headings(c);
                            Some(v.insert(CachedTarget { headings }))
                        }
                        Err(_) => {
                            if source_aware_is_dir(repo_root, source, &resolved, git_reader) {
                                None
                            } else {
                                // Missing file diagnostic with bare-path hint.
                                let first = Path::new(&link.path).components().next();
                                let is_explicit = matches!(
                                    first,
                                    Some(std::path::Component::CurDir)
                                        | Some(std::path::Component::ParentDir)
                                );
                                let is_bare = !link.path.starts_with('/') && !is_explicit;

                                let message = if is_bare {
                                    if source_aware_exists(repo_root, source, &link.path, git_reader) {
                                        format!(
                                            "File `{}` not found at page-relative path.\n\
                                             If you meant a repo-relative path, use `/{}` instead.",
                                            link.path, link.path
                                        )
                                    } else {
                                        format!("File `{}` not found.", link.path)
                                    }
                                } else {
                                    format!("File `{}` not found.", link.path)
                                };
                                diagnostics.push(CheckDiagnostic {
                                    kind: "broken_link".into(),
                                    file: path.display().to_string(),
                                    line: link.source_line,
                                    message,
                                });
                                continue;
                            }
                        }
                    }
                }
            };

            // Anchor validation: heading slugs only — line-range links left
            // the loop above (the drift pass owns them).
            if let Some(anchor) = anchor_of(&link.original_href)
                && !anchor.is_empty()
                && let Some(ref cached) = cached
            {
                // Non-line-range anchor: validate as heading slug.
                let decoded_anchor = percent_decode(anchor);
                if !resolve_heading(&decoded_anchor, &cached.headings) {
                    diagnostics.push(CheckDiagnostic {
                        kind: "broken_anchor".into(),
                        file: path.display().to_string(),
                        line: link.source_line,
                        message: format!("Heading `#{anchor}` not found in `{}`.", link.path),
                    });
                }
            }
        }
    }

    // Soft check that git is callable.
    let _ = resolve_ref(repo_root, "HEAD");

    // ── Drift pass ─────────────────────────────────────────────────────────────
    //
    // The sole authority for line-range links (plan Decision 7): each range
    // link is classified against its page's anchor epoch and reported with
    // one of the new kinds. `--fix` runs its own drift phase (Phase 2), so
    // the pass is off during fix-mode pre/post checks for now.
    if drift_pass {
        let drift_diags = collect_drift_diagnostics(
            files,
            repo_root,
            source,
            git_reader,
            content_cache,
        )?;
        diagnostics.extend(drift_diags);
    }

    Ok(diagnostics)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialize tests that share process-level state (e.g. PATH or env vars).
    static PATH_MUTEX: Mutex<()> = Mutex::new(());

    fn diag(kind: &str, line: usize) -> CheckDiagnostic {
        CheckDiagnostic {
            kind: kind.into(),
            file: "a.md".into(),
            line,
            message: "boom".into(),
        }
    }

    #[test]
    fn render_diagnostics_has_no_trailing_separator() {
        let rendered = render_diagnostics(&[diag("link_drift", 1), diag("broken_link", 2)]);
        assert!(
            !rendered.ends_with("---\n\n") && !rendered.trim_end().ends_with("---"),
            "separator must not appear after the last error: {rendered:?}"
        );
        assert_eq!(
            rendered.matches("\n---\n").count(),
            1,
            "separator must appear exactly once, between the two errors: {rendered:?}"
        );
        assert!(rendered.starts_with("Error: Link Drift\n"));
    }

    #[test]
    fn render_diagnostics_single_has_no_separator() {
        let rendered = render_diagnostics(&[diag("broken_link", 9)]);
        assert!(
            !rendered.contains("---"),
            "single error must have no separator: {rendered:?}"
        );
    }

    #[test]
    fn render_diagnostics_empty_is_empty() {
        assert_eq!(render_diagnostics(&[]), "");
    }

    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let repo = TestRepo { dir };
            repo.git(&["init"]);
            repo.git(&["checkout", "-b", "main"]);
            // Persist identity in the repo's local config so any git
            // invocation that does not inherit our per-call env vars still has
            // a committer. Real environments always have a git identity.
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "user.name", "Test Author"]);
            repo
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn create_file(&self, path: &str, content: &str) {
            let full = self.dir.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create_dir_all");
            }
            fs::write(&full, content).expect("write file");
        }

        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-m", message]);
        }

        fn git(&self, args: &[&str]) {
            let output = Command::new("git")
                .current_dir(self.dir.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test Committer")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("spawn git");
            assert!(
                output.status.success(),
                "git {:?} failed:\n{}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn make_wiki_page(title: &str, body: &str) -> String {
        format!("---\ntitle: {title}\nsummary: A page about {title}.\n---\n{body}")
    }

    /// A wiki page carrying a `links-reviewed:` value — the drift pass's
    /// certified-page shape.
    fn make_wiki_page_with_links_reviewed(title: &str, body: &str, value: &str) -> String {
        format!(
            "---\ntitle: {title}\nsummary: A page about {title}.\nlinks-reviewed: {value}\n---\n{body}"
        )
    }

    #[test]
    fn valid_pages_exit_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", &make_wiki_page("Page", "No links here."));
        repo.commit("add page");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn title_collision_exit_1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/a.md", &make_wiki_page("Shared", ""));
        repo.create_file("wiki/b.md", &make_wiki_page("Shared", ""));
        repo.commit("add pages");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn file_without_frontmatter_is_not_discovered() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("wiki/page.md", "# Just a heading\n\nNo frontmatter.");
        repo.commit("add page");

        // File has no frontmatter fence → not a wiki candidate → not discovered
        // → empty corpus → exit 2.
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 2);
    }

    #[test]
    fn missing_link_target_emits_broken_link() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [missing](/src/missing.rs)."),
        );
        repo.commit("add page");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().any(|d| d.kind == "broken_link"),
            "expected broken_link: {diags:?}"
        );
    }

    #[test]
    fn heading_anchor_resolves_when_present() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("add pages");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().all(|d| d.kind != "broken_anchor"),
            "anchor must resolve via slug: {diags:?}"
        );
    }

    #[test]
    fn missing_heading_anchor_emits_broken_anchor() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Introduction\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#nonexistent)."),
        );
        repo.commit("add pages");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().any(|d| d.kind == "broken_anchor"),
            "expected broken_anchor: {diags:?}"
        );
    }

    #[test]
    fn directory_link_is_valid() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/lib.rs", "fn main() {}");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [src](/src/) for details."),
        );
        repo.commit("add files");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn mailto_link_does_not_produce_broken_link() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "Contact [us](mailto:someone@example.com)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], repo.path()).expect("collect");
        assert!(
            diagnostics.iter().all(|d| d.kind != "broken_link"),
            "mailto: links must not produce broken_link: {diagnostics:?}"
        );
    }

    // ── Drift pass tests ──────────────────────────────────────────────────────

    /// A page carrying a line-range link but no `links-reviewed:` field
    /// anywhere fails closed with `anchor_epoch_missing` (plan Decision 5
    /// step 1): the check cannot vouch for the link, so it must not pass.
    #[test]
    fn range_link_without_field_emits_anchor_epoch_missing() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [code](/src/code.rs#L1-L1)."),
        );
        repo.commit("add files");

        let diagnostics = collect(&[], repo.path()).expect("collect");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.kind == "anchor_epoch_missing"),
            "expected anchor_epoch_missing: {diagnostics:?}"
        );
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 1);
    }

    /// A certified page whose link range overhangs its target's line count is
    /// `link_broken` — the drift engine's `Broken` replaces the old generic
    /// `broken_anchor` (plan Decision 7: the drift pass is the sole authority
    /// for line-range links).
    #[test]
    fn line_range_out_of_bounds_emits_link_broken() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page_with_links_reviewed(
                "Page",
                "See [code](/src/code.rs#L1-L1).",
                "1",
            ),
        );
        repo.commit("certify the link");
        // The href now overhangs the target; the field value is unchanged, so
        // the anchor epoch is the certification commit.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page_with_links_reviewed(
                "Page",
                "See [code](/src/code.rs#L100-L200).",
                "1",
            ),
        );

        let diagnostics = collect(&[], repo.path()).expect("collect");
        assert!(
            diagnostics.iter().any(|d| d.kind == "link_broken"),
            "expected link_broken: {diagnostics:?}"
        );
    }

    /// `--source=index` must validate staged content; broken link staged but
    /// worktree clean → must report from index. The drift pass reads the
    /// field from the index blob and walks the same history either way.
    #[test]
    fn source_index_validates_staged_broken_when_worktree_clean() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/code.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page_with_links_reviewed(
                "Page",
                "See [code](/src/code.rs#L1-L1).",
                "1",
            ),
        );
        repo.commit("certified baseline");

        // Stage a version whose range overhangs the target; restore the
        // worktree to the healthy version.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page_with_links_reviewed(
                "Page",
                "See [code](/src/code.rs#L100-L100).",
                "1",
            ),
        );
        repo.git(&["add", "wiki/page.md"]);
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page_with_links_reviewed(
                "Page",
                "See [code](/src/code.rs#L1-L1).",
                "1",
            ),
        );

        let diags_wt = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect wt");
        assert!(
            diags_wt.is_empty(),
            "worktree should be clean, got: {:?}",
            diags_wt
        );

        let diags_idx = collect_with_source(&[], repo.path(), crate::index::DocSource::Index)
            .expect("collect idx");
        assert!(
            diags_idx.iter().any(|d| d.kind == "link_broken"),
            "index should see staged broken link, got: {:?}",
            diags_idx
        );
    }

    /// `--source=head` consults the working tree via `abs.is_dir()` when
    /// `read_via_source` fails on a directory target, producing a false
    /// `broken_link` diagnostic for directories that exist at HEAD but were
    /// deleted from the worktree.
    ///
    /// MUST FAIL against the current unfixed code.  See hypothesis H1 in
    /// the card: `abs.is_dir()` at line 611 always checks the filesystem,
    /// ignoring the `--source` mode.
    #[test]
    fn source_head_does_not_emit_broken_link_for_committed_directory() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // A wiki page that links to a directory.
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [docs](./docs/)."),
        );
        // A file inside the directory so git tracks it.
        repo.create_file(
            "wiki/docs/readme.md",
            &make_wiki_page("Docs", "Documentation."),
        );
        repo.commit("add pages and docs directory");

        // Delete the directory from the worktree.
        fs::remove_dir_all(repo.path().join("wiki/docs")).expect("remove docs dir");

        // Validate against HEAD — the directory exists at HEAD, so link is valid.
        let diags = collect_with_source(&[], repo.path(), crate::index::DocSource::Head)
            .expect("collect");
        assert!(
            diags.iter().all(|d| d.kind != "broken_link"),
            "link to directory at HEAD must not emit broken_link: {diags:?}"
        );
    }

    // ── Fix pass integration tests (all #[ignore] until fix logic is implemented) ──

    /// Fix 1: when a link target was renamed in git, --fix rewrites the path.
    ///
    /// Plain-path href: line-range links are the drift phase's sole authority,
    /// and its `Broken` routing into `BrokenLinkRename` lands in Phase 2 (plan
    /// Decision 6) with its own tests.
    #[test]
    fn fix1_broken_link_rewrites_renamed_path() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/old.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [old](/src/old.rs)."),
        );
        repo.commit("baseline");

        // Rename the target file.
        repo.git(&["mv", "src/old.rs", "src/new.rs"]);
        // (do not commit so worktree sees the rename in the index)

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,  // fix
            false, // fix_dry_run
            false, // print_applied
        )
        .expect("run");

        let content = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");
        assert!(
            content.contains("/src/new.rs"),
            "expected link rewritten to new path, got:\n{content}"
        );
        assert_eq!(code, 0, "expected exit 0 after fix");
    }

    /// Fix 1 skip: when the rename target was deleted (not moved), skip the fix.
    ///
    /// Phase 1 interim: the href is a plain path — line-range links are the
    /// drift pass's sole authority, and its `Broken` routing into
    /// `BrokenLinkRename` lands in Phase 2 (plan Decision 6).
    #[test]
    fn fix1_skips_when_target_deleted() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/gone.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [gone](/src/gone.rs)."),
        );
        repo.commit("baseline");

        repo.git(&["rm", "src/gone.rs"]);

        let diags = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect");
        // A deletion is not a rename; fix should produce a SkippedFix.
        // Post-fix diagnostics must still contain broken_link.
        assert!(
            diags.iter().any(|d| d.kind == "broken_link"),
            "expected broken_link for deleted target: {diags:?}"
        );
    }

    /// Fix 1 skip: when a path maps to multiple rename targets, skip (ambiguous).
    ///
    /// Phase 1 interim: plain-path href — range-link `Broken` routing into
    /// `BrokenLinkRename` lands in Phase 2 (plan Decision 6).
    #[test]
    fn fix1_skips_when_rename_ambiguous() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/shared.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [shared](/src/shared.rs)."),
        );
        repo.commit("baseline");

        // Simulate ambiguity: two copies of the file exist under different names.
        repo.create_file("src/copy_a.rs", "fn a() {}\n");
        repo.create_file("src/copy_b.rs", "fn a() {}\n");
        repo.git(&["rm", "src/shared.rs"]);

        // With two possible rename destinations, fix must not apply automatically.
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");
        // Link is still broken → exit 1.
        assert_eq!(code, 1);
    }

    /// Fix 3: when an inbound link uses an aliased slug, --fix rewrites it to the
    /// canonical title slug so the alias entry can later be retired.
    #[test]
    fn fix3_alias_rewrites_to_canonical_slug() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Overview\nsummary: s.\naliases:\n  - introduction\n---\n## Overview\n\nbody\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("target.md#overview"),
            "expected anchor rewritten to canonical slug `overview`, got:\n{content}"
        );
        assert!(!content.contains("#introduction"));
        assert_eq!(code, 0);
    }

    /// Fix 3 skip: when the target page lists an alias but has no headings at
    /// all, Fix #3 declines rather than rewriting to a non-existent anchor.
    #[test]
    fn fix3_skips_when_target_has_no_headings() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Title is "Overview" and an alias is listed, but the page body has
        // no headings whatsoever — no canonical destination exists.
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Overview\nsummary: s.\naliases:\n  - introduction\n---\nJust prose, no headings.\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("target.md#introduction"),
            "expected anchor unchanged (skip), got:\n{content}"
        );
        // Post-fix the diagnostic should still be present (link still broken).
        // Exit 1 because broken_anchor persists.
        assert_eq!(code, 1);
    }

    /// Fix 3 fallback: title slug does not match a heading, but the page has a
    /// first H1 — rewrite to the H1's slug.
    #[test]
    fn fix3_falls_back_to_first_h1_when_title_mismatch() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Title is "Authentication" but H1 is "Auth & Authorization".
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Authentication\nsummary: s.\naliases:\n  - introduction\n---\n# Auth & Authorization\n\nbody\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        // First H1 is "Auth & Authorization"; github_slug gives "auth--authorization"
        // (the ampersand drops, leaving the two surrounding spaces as hyphens).
        assert!(
            content.contains("target.md#auth--authorization"),
            "expected anchor rewritten to first-H1 slug, got:\n{content}"
        );
        assert!(!content.contains("#introduction"));
        assert_eq!(code, 0);
    }

    /// Fix 3 fallback: no heading matches the title and there is no H1 —
    /// rewrite to the first heading at any level.
    #[test]
    fn fix3_falls_back_to_first_heading_when_no_h1() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Title is "Overview" but only a `## Body` heading exists — matches
        // the reproducer in the card.
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Overview\nsummary: s.\naliases:\n  - introduction\n---\n## Body\n\nbody\n",
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [intro](target.md#introduction)."),
        );
        repo.commit("baseline");

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("target.md#body"),
            "expected anchor rewritten to first-heading slug `body`, got:\n{content}"
        );
        assert!(!content.contains("#introduction"));
        assert_eq!(code, 0);
    }

    /// Fix 5: when a heading was renamed in place (same section position), --fix
    /// updates the anchor in all linking wiki pages.
    #[test]
    fn fix5_heading_rename_same_position_rewrites() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Old Heading\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [target](target.md#old-heading)."),
        );
        repo.commit("baseline");

        // Rename the heading.
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## New Heading\n\nbody\n"),
        );

        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("run");

        let content =
            std::fs::read_to_string(repo.path().join("wiki/source.md")).expect("read source");
        assert!(
            content.contains("#new-heading"),
            "expected anchor updated to new-heading, got:\n{content}"
        );
        assert_eq!(code, 0);
    }

    /// Fix 5 skip: when a heading was split into two or more headings, do not apply.
    #[test]
    fn fix5_skips_when_heading_split() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Combined\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [target](target.md#combined)."),
        );
        repo.commit("baseline");

        // Split into two headings.
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Part One\n\nbody\n\n## Part Two\n\nbody\n"),
        );

        let diags = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect");
        assert!(
            !diags.is_empty(),
            "expected diagnostics when heading was split: {diags:?}"
        );
    }

    /// Fix 5 skip: when the heading was removed and other headings were restructured,
    /// do not attempt a fix.
    #[test]
    fn fix5_skips_when_heading_restructured() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Section A\n\nbody\n## Section B\n\nbody\n"),
        );
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [target](target.md#section-a)."),
        );
        repo.commit("baseline");

        // Restructure: remove Section A and rename Section B.
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Renamed B\n\nbody\n"),
        );

        let diags = collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
            .expect("collect");
        assert!(
            !diags.is_empty(),
            "expected diagnostics when heading was restructured: {diags:?}"
        );
    }

    /// Running --fix twice must produce no further rewrites on the second pass.
    #[test]
    fn wiki_check_fix_is_idempotent() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/old.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [old](/src/old.rs#L1)."),
        );
        repo.commit("baseline");

        repo.git(&["mv", "src/old.rs", "src/new.rs"]);

        // First pass.
        run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("first fix pass");

        let content_after_first =
            std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");

        // Second pass.
        run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            true,
            false,
            false,
        )
        .expect("second fix pass");

        let content_after_second =
            std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");

        assert_eq!(
            content_after_first, content_after_second,
            "--fix must be idempotent: file changed on second pass"
        );
    }

    /// --fix must be rejected when --source is not worktree.
    #[test]
    fn wiki_check_fix_rejects_non_worktree_source() {
        // This test validates the CLI guard in main.rs; since tests call
        // commands::check::run directly (bypassing main.rs), we verify the
        // documented contract by calling run() with a non-worktree source and
        // fix=true, and asserting that the function itself does not mutate any
        // files. (The main.rs guard prints an error and returns Ok(2) before
        // reaching this function.)
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("src/old.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [old](/src/old.rs#L1)."),
        );
        repo.commit("baseline");

        let original =
            std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");

        // Calling run directly with Index source and fix=true; no files should change.
        let _code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::Index,
            true,
            false,
            false,
        )
        .expect("run");

        let after = std::fs::read_to_string(repo.path().join("wiki/page.md")).expect("read page");
        assert_eq!(
            original, after,
            "file must not be mutated when source != worktree"
        );
    }

    /// Finding 1 regression: a genuine infrastructure error during `discover_files`
    /// (not the empty-corpus condition) must exit 2 in `--fix` mode, not degrade to
    /// an empty-corpus success.  We use `DocSource::Index` on a repo with no git
    /// index at all (no `git add` ever run) so that `list_paths` fails with a real
    /// git error; this is distinct from the empty-corpus condition (Ok(vec![])).
    ///
    /// The main.rs CLI guard rejects `--fix --source=index`, so we call `run`
    /// directly — the same path a hook or test exercises.
    #[test]
    fn fix_mode_genuine_discover_error_exits_2_not_0() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // Brand-new repo; no `git add` → no .git/index → Index.list_paths fails.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path();
        // git init without any subsequent git add so the index file is absent.
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root)
            .output()
            .expect("git init");
        // Place a wiki page on disk so this is not the "no .md files" case.
        std::fs::create_dir_all(root.join("wiki")).expect("mkdir wiki");
        std::fs::write(
            root.join("wiki/page.md"),
            make_wiki_page("Page", "No links."),
        )
        .expect("write page");

        // fix=true, source=Index → discover_files calls list_paths which fails
        // (no .git/index).  Must exit 2, not 0.
        let code = run(
            &[],
            false,
            root,
            root,
            false,
            crate::index::DocSource::Index,
            true, // fix
            false,
            false,
        )
        .expect("run");
        assert_eq!(
            code, 2,
            "genuine infrastructure error in --fix mode must exit 2, not 0"
        );
    }

    /// Finding 1 regression (non-fix path): empty corpus in non-fix mode still
    /// exits 2 with "no wiki pages found" after the discover_files contract change.
    #[test]
    fn non_fix_empty_corpus_still_exits_2() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // No wiki pages at all.
        let code = run(
            &[],
            false,
            repo.path(),
            repo.path(),
            false,
            crate::index::DocSource::WorkingTree,
            false,
            false,
            false,
        )
        .expect("run");
        assert_eq!(code, 2, "empty corpus must exit 2 in non-fix mode");
    }

    /// Reproduction: a percent-encoded href like `./my%20file.md` must resolve to
    /// the on-disk file `my file.md` without emitting `broken_link`.
    #[test]
    fn percent_encoded_href_to_existing_file_resolves() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Create the target file with a space in its name.
        repo.create_file("wiki/my file.md", &make_wiki_page("My File", "Content.\n"));
        // Source page links with a percent-encoded href.
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [file](./my%20file.md)."),
        );
        repo.commit("add pages");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().all(|d| d.kind != "broken_link"),
            "percent-encoded href to existing file must not emit broken_link: {diags:?}"
        );
    }

    /// Reproduction: a percent-encoded heading anchor like `#caf%C3%A9` must
    /// resolve to the heading `## Café` without emitting `broken_anchor`.
    #[test]
    fn percent_encoded_heading_anchor_resolves() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        // Target page with a non-ASCII heading.
        repo.create_file(
            "wiki/target.md",
            &make_wiki_page("Target", "## Café\n\nbody\n"),
        );
        // Source page links with a percent-encoded anchor.
        repo.create_file(
            "wiki/source.md",
            &make_wiki_page("Source", "See [cafe](target.md#caf%C3%A9)."),
        );
        repo.commit("add pages");

        let diags = collect(&[], repo.path()).expect("collect");
        assert!(
            diags.iter().all(|d| d.kind != "broken_anchor"),
            "percent-encoded heading anchor must resolve: {diags:?}"
        );
    }

    /// End-to-end: the drift pass applies suffix salvage to a stale href
    /// prefix (plan Decision 5). A certified page whose link gains a
    /// nonexistent directory prefix classifies Healthy against the salvaged
    /// target — the read-only check reports nothing.
    #[test]
    fn drift_pass_salvages_stale_href_prefix() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();
        repo.create_file("target.rs", "fn a() {}\n");
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page_with_links_reviewed(
                "Page",
                "See [code](../target.rs#L1-L1).",
                "1",
            ),
        );
        repo.commit("certified baseline");

        // The href gains a directory prefix that does not exist; suffix
        // salvage falls back to `target.rs` at the repo root.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page_with_links_reviewed(
                "Page",
                "See [code](dir/target.rs#L1-L1).",
                "1",
            ),
        );

        let diagnostics = collect(&[], repo.path()).expect("collect");
        assert!(
            diagnostics.is_empty(),
            "salvaged link must classify Healthy: {diagnostics:?}"
        );
    }

    /// Reproduction: when `--source=head` is used and a bare-path link target
    /// exists at HEAD but not in the worktree, the repo-relative path hint
    /// ("If you meant a repo-relative path") must still appear because the file
    /// IS present in the source being validated.
    ///
    /// At line 625, `repo_relative_abs.exists()` consults the worktree
    /// regardless of `--source`, so under `--source=head` a file deleted from
    /// the worktree produces a generic "File not found" message instead of the
    /// repo-relative hint.
    ///
    /// MUST FAIL against the current unfixed code: the assertion that the
    /// hint message IS present will fail because the worktree check returns
    /// false for the deleted file.
    #[test]
    fn source_head_bare_path_hint_uses_head_not_worktree() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();

        // Wiki page with a bare-path link: `[utils](src/utils.rs)` resolves
        // page-relative to `wiki/src/utils.rs`, which doesn't exist at HEAD.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page("Page", "See [utils](src/utils.rs)."),
        );
        // Repo-relative `src/utils.rs` exists at HEAD.
        repo.create_file("src/utils.rs", "fn util() {}\n");
        repo.commit("add page and src/utils.rs");

        // Delete src/utils.rs from the worktree — it still exists at HEAD.
        std::fs::remove_file(repo.path().join("src/utils.rs")).expect("remove file");

        let diags =
            collect_with_source(&[], repo.path(), crate::index::DocSource::Head).expect("collect");

        let bl: Vec<_> = diags.iter().filter(|d| d.kind == "broken_link").collect();
        assert_eq!(bl.len(), 1, "expected exactly one broken_link: {diags:?}");
        assert!(
            bl[0].message.contains("If you meant a repo-relative path"),
            "expected hint about repo-relative path under --source=head (file exists at HEAD), \
             got: {}",
            bl[0].message
        );
    }

    /// A link whose target is wikiignored must not produce `broken_link` or
    /// `broken_anchor` diagnostics, even when the target file is missing or has
    /// an out-of-bounds line range.
    #[test]
    fn wikiignored_link_target_exempt_from_broken_link_and_broken_anchor() {
        let _guard = PATH_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let repo = TestRepo::new();

        // Wiki page links into a wikiignored file that does not exist.
        repo.create_file(
            "wiki/page.md",
            &make_wiki_page(
                "Page",
                "See [secret](/src/secret.rs#L1-L5) and [also](/src/secret.rs).",
            ),
        );
        // Wikiignore the target; do NOT create the target file so it would
        // trigger broken_link / broken_anchor if the exemption were absent.
        repo.create_file(".wiki/.wikiignore", "src/secret.rs\n");
        repo.commit("add page with wikiignored link target");

        let diags =
            collect_with_source(&[], repo.path(), crate::index::DocSource::WorkingTree)
                .expect("collect");

        let bad: Vec<_> = diags
            .iter()
            .filter(|d| d.kind == "broken_link" || d.kind == "broken_anchor")
            .collect();
        assert!(
            bad.is_empty(),
            "wikiignored link target must not raise broken_link or broken_anchor: {bad:?}",
        );
    }
}
