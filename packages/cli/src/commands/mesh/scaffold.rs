//! `wiki scaffold` end-to-end pipeline.
//!
//! Discover wiki files, parse their fragment links, and emit a markdown
//! document (or JSON) of `git mesh add` / `git mesh why` commands — one mesh
//! per link.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use miette::{IntoDiagnostic, Result};
use regex::Regex;
use serde::Serialize;

use crate::commands::{discover_files, resolve_link_path};
use crate::index::DocSource;
use crate::parser::{LinkKind, parse_fragment_links};
use crate::wiki_config::WikiInfo;

/// Read `path` from the chosen [`DocSource`], routing non-worktree reads
/// through [`DocSource::read`] so the content snapshot matches the discovery
/// snapshot.
fn read_via_source(path: &Path, repo_root: &Path, source: DocSource) -> std::io::Result<String> {
    match source {
        DocSource::WorkingTree => fs::read_to_string(path),
        DocSource::Index | DocSource::Head => {
            let path_rel = path
                .strip_prefix(repo_root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            match source.read(repo_root, &path_rel) {
                Ok(Some(s)) => Ok(s),
                Ok(None) => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{path_rel} not present in source {source:?}"),
                )),
                Err(e) => Err(std::io::Error::other(e.to_string())),
            }
        }
    }
}

// ── Parse-error types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum ParseErrorKind {
    /// File does not start with `---\n`.
    NoFrontmatter,
    /// Frontmatter present, no `title:` key.
    MissingTitle,
    /// `title:` present, value empty/whitespace.
    EmptyTitle,
    /// IO error or invalid UTF-8 — message captured.
    Unreadable(String),
    /// Starts with `---` but regex rejected it (BOM, CRLF, no closing fence, etc.).
    Malformed,
}

#[derive(Debug, Clone)]
pub(crate) struct ParseError {
    pub(crate) path: String,
    pub(crate) kind: ParseErrorKind,
}

// ── JSON output types ─────────────────────────────────────────────────────────

/// Top-level JSON output for `wiki scaffold --format json`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScaffoldOutput {
    schema_version: u32,
    parse_errors: Vec<ParseErrorJson>,
    pages: Vec<PageJson>,
}

/// JSON representation of a parse error.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ParseErrorJson {
    path: String,
    category: ParseErrorCategory,
    message: String,
}

/// Machine-stable parse-error category tags.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ParseErrorCategory {
    NoFrontmatter,
    MissingTitle,
    EmptyTitle,
    Unreadable,
    MalformedFrontmatter,
}

/// JSON representation of a per-page section.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageJson {
    path: String,
    title: String,
    meshes: Vec<MeshJson>,
}

/// JSON representation of one mesh entry.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeshJson {
    slug: String,
    heading_chain: Vec<String>,
    anchors: Vec<AnchorJson>,
}

/// JSON representation of a structured anchor.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnchorJson {
    path: String,
    start_line: u32,
    end_line: u32,
}

impl ParseErrorCategory {
    fn from_kind(kind: &ParseErrorKind) -> Self {
        match kind {
            ParseErrorKind::NoFrontmatter => ParseErrorCategory::NoFrontmatter,
            ParseErrorKind::MissingTitle => ParseErrorCategory::MissingTitle,
            ParseErrorKind::EmptyTitle => ParseErrorCategory::EmptyTitle,
            ParseErrorKind::Unreadable(_) => ParseErrorCategory::Unreadable,
            ParseErrorKind::Malformed => ParseErrorCategory::MalformedFrontmatter,
        }
    }
}

impl ParseErrorKind {
    pub(crate) fn reason(&self) -> String {
        match self {
            ParseErrorKind::NoFrontmatter => {
                "no frontmatter block — file does not start with `---`".to_string()
            }
            ParseErrorKind::MissingTitle => {
                "frontmatter present but `title:` is missing".to_string()
            }
            ParseErrorKind::EmptyTitle => "frontmatter present but `title:` is empty".to_string(),
            ParseErrorKind::Unreadable(msg) => format!("file could not be read: {msg}"),
            ParseErrorKind::Malformed => {
                "malformed frontmatter — could not parse `title`".to_string()
            }
        }
    }
}

use super::augment::{AugmentedLink, augment};
use super::draft::{self, MeshDraft};
use super::group;
use super::render;

/// Run the `wiki scaffold` subcommand.
pub fn run(
    globs: &[String],
    json: bool,
    wiki_roots: &[PathBuf],
    wiki_infos: &[WikiInfo],
    repo_root: &Path,
    source: crate::index::DocSource,
) -> Result<i32> {
    let mut files: Vec<PathBuf> = Vec::new();
    for wiki_root in wiki_roots {
        // Per-iteration discovery operates against this wiki's root: a glob
        // that resolves outside this root may legitimately yield zero files
        // here while still matching under another iteration. Treat that as
        // empty rather than failing the whole scaffold run; the caller
        // surfaces a real "no wiki pages found" error only if every
        // iteration produces zero files (see the post-loop check below).
        let discovered = match discover_files(globs, wiki_root, repo_root, source) {
            Ok(v) => v,
            Err(e) => {
                if e.to_string().contains("no wiki pages found") {
                    Vec::new()
                } else {
                    return Err(e);
                }
            }
        };
        // Filter test fixtures: `wiki check` legitimately scans them, but a
        // scaffold run that materializes mesh commands for a test wiki would
        // pollute the repo's mesh state on the first commit. Scoped to scaffold.
        for f in discovered {
            let s = f.to_string_lossy();
            if !s.contains("/tests/fixtures/") && !s.contains("\\tests\\fixtures\\") {
                files.push(f);
            }
        }
    }
    // Deduplicate across wikis (each discover_files call already deduplicates
    // internally, but the same file may be matched through multiple wiki roots).
    files.sort();
    files.dedup();

    let mut all_inputs: Vec<LinkInput> = Vec::new();
    for file in &files {
        let content = match read_via_source(file, repo_root, source) {
            Ok(s) => s,
            Err(_) => {
                // Unreadable files are surfaced via parse_errors (classify_frontmatter
                // records ParseErrorKind::Unreadable independently). Skip from the
                // link pipeline.
                continue;
            }
        };
        let raw_links = parse_fragment_links(&content);
        let augmented = augment(&raw_links, &content);
        // Filter to internal links with a parsed line range — mirrors the JS
        // which skips URL-scheme links and links lacking `#`.
        for aug in augmented {
            if aug.link.kind != LinkKind::Internal {
                continue;
            }
            if aug.link.start_line.is_none() {
                continue;
            }
            all_inputs.push(LinkInput {
                wiki_file: file.clone(),
                augmented: aug,
            });
        }
    }

    // Build per-source frontmatter map (title) keyed by absolute path,
    // and accumulate parse errors for source files.
    let mut wiki_meta_cache: std::collections::HashMap<PathBuf, FileMeta> =
        std::collections::HashMap::new();
    let mut parse_errors: Vec<ParseError> = Vec::new();
    for f in &files {
        let (meta, err_kind) = classify_frontmatter(f, repo_root, source);
        if let Some(kind) = err_kind {
            let rel = path_relative_to(f, repo_root);
            parse_errors.push(ParseError { path: rel, kind });
        }
        wiki_meta_cache.insert(f.clone(), meta);
    }
    parse_errors.sort_by(|a, b| a.path.cmp(&b.path));

    // Build the page-title lookup keyed by repo-root-relative path strings.
    let mut page_titles: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    // Parallel map: per-page slug namespace context (owning wiki + subdir).
    let mut page_namespaces: std::collections::HashMap<String, PageNamespace> =
        std::collections::HashMap::new();
    for f in &files {
        let rel = path_relative_to(f, repo_root);
        let title = wiki_meta_cache.get(f).and_then(|m| m.title.clone());
        let fm_ns = wiki_meta_cache.get(f).and_then(|m| m.namespace.clone());
        page_titles.insert(rel.clone(), title);
        let ns = resolve_page_namespace(f, repo_root, wiki_infos, fm_ns.as_deref());
        page_namespaces.insert(rel, ns);
    }

    // Collect parse-error paths for exclusion from pages output.
    let parse_error_paths: std::collections::HashSet<String> =
        parse_errors.iter().map(|e| e.path.clone()).collect();

    // ── Unified build/group pipeline (both modes) ─────────────────────────
    // Trim heading chains once here so both renderers consume pre-trimmed data.
    let mut consolidated = build_meshes(&all_inputs, repo_root, &page_namespaces);
    trim_chains_in_place(&mut consolidated, &page_titles);

    // Resolve slug collisions against both already-assigned slugs in this run
    // and any meshes that already live in the repo. The probe shells out to
    // `git mesh <slug>` per candidate; missing `git-mesh` is treated as "no
    // collisions" so scaffold still works without it. The probe runs after
    // heading-chain trimming so the page-title duplicates we just dropped
    // aren't reintroduced as qualifiers.
    let probe = |slug: &str| mesh_exists(repo_root, slug);
    resolve_slug_collisions(&mut consolidated, &page_titles, &probe);

    if json {
        let parse_errors_json: Vec<ParseErrorJson> = parse_errors
            .iter()
            .map(|e| ParseErrorJson {
                path: e.path.clone(),
                category: ParseErrorCategory::from_kind(&e.kind),
                message: e.kind.reason(),
            })
            .collect();
        let pages = build_pages_json(&consolidated, &page_titles, &parse_error_paths);
        let output = ScaffoldOutput {
            schema_version: 1,
            parse_errors: parse_errors_json,
            pages,
        };
        let s = serde_json::to_string_pretty(&output).into_diagnostic()?;
        println!("{s}");
        return Ok(0);
    }

    // ── Markdown mode ─────────────────────────────────────────────────────
    if all_inputs.is_empty() {
        print!("{}", render::render_empty_markdown(&parse_errors));
        return Ok(0);
    }

    let rendered = render::render_markdown(&consolidated, &page_titles, &parse_errors, &parse_error_paths);
    print!("{rendered}");
    Ok(0)
}

/// Build the JSON page list from consolidated drafts, excluding pages whose
/// paths appear in `parse_error_paths` (schema must be disjoint).
fn build_pages_json(
    drafts: &[MeshDraft],
    page_titles: &std::collections::HashMap<String, Option<String>>,
    parse_error_paths: &std::collections::HashSet<String>,
) -> Vec<PageJson> {
    // Group by page in first-occurrence order.
    let mut page_order: Vec<String> = Vec::new();
    let mut by_page: std::collections::HashMap<String, Vec<&MeshDraft>> =
        std::collections::HashMap::new();
    for d in drafts {
        if parse_error_paths.contains(&d.page_path) {
            continue;
        }
        if !by_page.contains_key(&d.page_path) {
            page_order.push(d.page_path.clone());
        }
        by_page.entry(d.page_path.clone()).or_default().push(d);
    }

    page_order
        .into_iter()
        .map(|page_path| {
            let title = page_titles
                .get(&page_path)
                .and_then(|t| t.clone())
                .unwrap_or_default();
            let page_drafts = by_page.get(&page_path).expect("tracked");
            let meshes = page_drafts
                .iter()
                .map(|d| {
                    // heading_chain was already trimmed once in trim_chains_in_place.
                    MeshJson {
                        slug: d.slug.clone(),
                        heading_chain: d.heading_chain.clone(),
                        anchors: d
                            .structured_anchors
                            .iter()
                            .map(|a| AnchorJson {
                                path: a.path.clone(),
                                start_line: a.start_line,
                                end_line: a.end_line,
                            })
                            .collect(),
                    }
                })
                .collect();
            PageJson {
                path: page_path,
                title,
                meshes,
            }
        })
        .collect()
}

/// Trim heading chains on all drafts in place. The leading chain entry is
/// dropped when it matches the page's frontmatter title after normalization.
/// This runs once after `build_meshes` so both renderers consume pre-trimmed data.
fn trim_chains_in_place(
    drafts: &mut [MeshDraft],
    page_titles: &std::collections::HashMap<String, Option<String>>,
) {
    for d in drafts.iter_mut() {
        let title = page_titles
            .get(&d.page_path)
            .and_then(|t| t.as_deref())
            .unwrap_or("");
        d.heading_chain = trim_heading_chain(&d.heading_chain, title);
    }
}

/// Trim the leading entry of `heading_chain` when it matches the page's
/// frontmatter `title` after normalization (strip inline markup, collapse
/// whitespace, case-insensitive compare). Returns the trimmed chain.
pub(crate) fn trim_heading_chain(chain: &[String], page_title: &str) -> Vec<String> {
    if chain.is_empty() {
        return Vec::new();
    }
    let normalized_title = normalize_heading_text(page_title);
    let normalized_first = normalize_heading_text(&chain[0]);
    if !normalized_title.is_empty() && normalized_first.eq_ignore_ascii_case(&normalized_title) {
        chain[1..].to_vec()
    } else {
        chain.to_vec()
    }
}

/// Normalize heading or title text for comparison: strip inline markup chars
/// (`*`, `_`, `` ` ``, `[`, `]`), collapse whitespace.
pub(crate) fn normalize_heading_text(s: &str) -> String {
    let stripped: String = s.chars().filter(|c| !"`*_[]".contains(*c)).collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Three-stage build/group/annotate pipeline that produces the final list of
/// meshes (in per-page declaration order) ready for shell rendering.
fn build_meshes(
    inputs: &[LinkInput],
    repo_root: &Path,
    page_namespaces: &std::collections::HashMap<String, PageNamespace>,
) -> Vec<MeshDraft> {
    // Group inputs by source page (preserving discovery order).
    let mut page_order: Vec<PathBuf> = Vec::new();
    let mut by_page: std::collections::HashMap<PathBuf, Vec<&LinkInput>> =
        std::collections::HashMap::new();
    for input in inputs {
        if !by_page.contains_key(&input.wiki_file) {
            page_order.push(input.wiki_file.clone());
        }
        by_page
            .entry(input.wiki_file.clone())
            .or_default()
            .push(input);
    }

    // Stage 1: per-page section grouping → one draft per section.
    let mut all_drafts: Vec<MeshDraft> = Vec::new();
    let mut page_spans: Vec<(usize, usize)> = Vec::with_capacity(page_order.len());
    for page in &page_order {
        let entries = by_page.get(page).expect("page tracked in order");
        let page_rel = path_relative_to(page, repo_root);

        // Group entries by (section_start, section_end) preserving first-occurrence order.
        let mut section_order: Vec<(u32, u32)> = Vec::new();
        let mut by_section: std::collections::HashMap<(u32, u32), Vec<&LinkInput>> =
            std::collections::HashMap::new();
        for entry in entries {
            let key = (
                entry.augmented.section_start_line,
                entry.augmented.section_end_line,
            );
            if !by_section.contains_key(&key) {
                section_order.push(key);
            }
            by_section.entry(key).or_default().push(entry);
        }

        type GroupTuple<'a> = (
            &'a AugmentedLink,
            u32,
            u32,
            Vec<String>,
            Vec<draft::StructuredAnchor>,
        );
        let mut groups_storage: Vec<GroupTuple<'_>> = Vec::with_capacity(section_order.len());
        for key in &section_order {
            let section_entries = by_section.get(key).expect("tracked");
            let leader = &section_entries[0].augmented;
            let mut seen: std::collections::HashSet<(String, u32, u32)> =
                std::collections::HashSet::new();
            let mut target_anchors: Vec<String> = Vec::new();
            let mut structured_targets: Vec<draft::StructuredAnchor> = Vec::new();
            for entry in section_entries {
                let link = &entry.augmented.link;
                let resolved = resolve_link_path(&link.path, &entry.wiki_file, repo_root);
                let anchor_rel = path_relative_to(&resolved, repo_root);
                let anchor_rel =
                    locate_existing_suffix(&anchor_rel, repo_root).unwrap_or(anchor_rel);
                let start = link.start_line.unwrap_or(0);
                let end = link.end_line.unwrap_or(start);
                let triple = (anchor_rel.clone(), start, end);
                if !seen.insert(triple) {
                    continue;
                }
                target_anchors.push(format!("{anchor_rel}#L{start}-L{end}"));
                structured_targets.push(draft::StructuredAnchor {
                    path: anchor_rel,
                    start_line: start,
                    end_line: end,
                });
            }
            groups_storage.push((leader, key.0, key.1, target_anchors, structured_targets));
        }
        let groups: Vec<draft::SectionGroup<'_>> = groups_storage
            .iter()
            .map(|(leader, s, e, ta, st)| draft::SectionGroup {
                leader,
                section_start: *s,
                section_end: *e,
                target_anchors: ta.clone(),
                structured_targets: st.clone(),
            })
            .collect();
        // Look up the owning wiki for slug derivation. A page should always
        // be in the map (every discovered file is registered above), but fall
        // back to a default-namespace empty-subdir context so a missing entry
        // can never panic — the slug still gets the `wiki/` prefix.
        let page_ns = page_namespaces
            .get(&page_rel)
            .cloned()
            .unwrap_or_default();
        let drafts = draft::build(&page_rel, &groups, repo_root, &page_ns);
        let start = all_drafts.len();
        all_drafts.extend(drafts);
        page_spans.push((start, all_drafts.len()));
    }

    // Stage 2: per-page consolidation. Identical-anchor-set siblings collapse
    // into one survivor; only then does the collision resolver (run from
    // [`run`] after heading-chain trimming) see contiguous slugs. Doing this
    // in the reverse order leaks suffix gaps (`foo`, `foo-3`, no `foo-2`)
    // into the footer's `git mesh commit` lines whenever consolidation
    // prunes a duplicate the dedup already suffixed.
    let mut consolidated: Vec<MeshDraft> = Vec::new();
    for (start, end) in page_spans {
        let page_drafts: Vec<MeshDraft> = all_drafts[start..end].to_vec();
        consolidated.extend(group::consolidate_within_page(page_drafts));
    }

    consolidated
}

/// Probe whether a mesh with `slug` already exists in `repo_root` by invoking
/// `git mesh <slug>` and inspecting the exit code. Treats any error (missing
/// `git-mesh` binary, non-mesh repo, signal-killed child) as "does not exist"
/// so scaffold remains usable in environments without `git-mesh` installed.
fn mesh_exists(repo_root: &Path, slug: &str) -> bool {
    Command::new("git")
        .args(["mesh", slug])
        .current_dir(repo_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Resolve slug collisions by progressively prepending semantic qualifiers
/// drawn from the section's heading chain and the page title.
///
/// Each draft starts with its base slug. If that slug collides with either
/// an earlier-assigned slug in this run or a pre-existing mesh (`mesh_exists`
/// returns `true`), the resolver tries successively longer qualifier sets:
///
/// 1. The immediate parent heading (`heading_chain.last()`), then grandparent,
///    great-grandparent, … each new heading prepended outer→inner so the slug
///    reads top-down like a path.
/// 2. After the heading chain is exhausted, the page's frontmatter title
///    (kebab-cased) is prepended ahead of the full chain.
/// 3. Only when *all* semantic qualifiers fail does the resolver fall back to
///    numeric `-2`, `-3`, … suffixes appended to the last unique semantic
///    candidate.
///
/// Duplicate candidate slugs (caused by [`draft::build_slug_with_qualifiers`]
/// dropping a reserved qualifier) are skipped so the search makes monotonic
/// progress instead of looping on the same string.
pub(crate) fn resolve_slug_collisions(
    drafts: &mut [MeshDraft],
    page_titles: &std::collections::HashMap<String, Option<String>>,
    mesh_exists: &dyn Fn(&str) -> bool,
) {
    let mut assigned: std::collections::HashSet<String> = std::collections::HashSet::new();
    for d in drafts.iter_mut() {
        // Inner→outer chain in kebab form, dropping empties and any entry
        // equal to the noun (the deepest entry in `heading_chain` is the
        // section heading itself, which already supplied the noun — using it
        // as a qualifier would just re-emit `wiki/foo/foo`).
        let chain_kebab: Vec<String> = d
            .heading_chain
            .iter()
            .map(|h| draft::kebab(h))
            .filter(|s| !s.is_empty() && s != &d.noun)
            .collect();
        let title_kebab = page_titles
            .get(&d.page_path)
            .and_then(|t| t.as_deref())
            .filter(|t| !t.is_empty())
            .map(draft::kebab)
            .filter(|s| !s.is_empty() && s != &d.noun);

        // Candidate qualifier sets to try, in priority order.
        let mut candidates: Vec<Vec<String>> = Vec::new();
        candidates.push(Vec::new());
        for k in 1..=chain_kebab.len() {
            let slice = &chain_kebab[chain_kebab.len() - k..];
            candidates.push(slice.to_vec());
        }
        if let Some(title) = &title_kebab {
            let mut with_title: Vec<String> = vec![title.clone()];
            with_title.extend(chain_kebab.iter().cloned());
            candidates.push(with_title);
        }

        let mut tried: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut last_unique: Option<String> = None;
        let mut resolved: Option<String> = None;
        for quals in &candidates {
            let slug = draft::build_slug_with_qualifiers(&d.page_ns, quals, &d.noun);
            if !tried.insert(slug.clone()) {
                continue;
            }
            last_unique = Some(slug.clone());
            if !assigned.contains(&slug) && !mesh_exists(&slug) {
                resolved = Some(slug);
                break;
            }
        }
        let final_slug = resolved.unwrap_or_else(|| {
            let base = last_unique.unwrap_or_else(|| d.slug.clone());
            let mut n: u32 = 2;
            loop {
                let cand = format!("{base}-{n}");
                if !assigned.contains(&cand) && !mesh_exists(&cand) {
                    break cand;
                }
                n += 1;
            }
        });
        assigned.insert(final_slug.clone());
        d.slug = final_slug;
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

struct LinkInput {
    wiki_file: PathBuf,
    augmented: AugmentedLink,
}

#[derive(Debug, Clone, Default)]
struct FileMeta {
    title: Option<String>,
    /// Frontmatter `namespace` field, when present. Used as the slug
    /// namespace for `.wiki.md` pages that live outside any wiki root.
    namespace: Option<String>,
}

/// Per-page slug context: which wiki namespace owns the page (`None` for the
/// default-namespace wiki) and the page's directory path relative to its
/// owning wiki root (forward slashes, no leading or trailing slash; empty
/// when the page sits directly at the wiki root).
#[derive(Debug, Clone, Default)]
pub(crate) struct PageNamespace {
    pub(crate) namespace: Option<String>,
    pub(crate) subdir: String,
}

/// Resolve which wiki owns a page and the page's directory within that wiki.
///
/// Pages under a `wiki.toml` tree inherit that wiki's namespace; the longest
/// containing root wins (a nested wiki shadows an outer one). Pages outside
/// every root fall back to their frontmatter `namespace` field (a `.wiki.md`
/// float opting into a peer wiki). When no enclosing root and no frontmatter
/// declaration apply, the page is treated as default-namespace with an empty
/// subdir — the slug becomes `wiki/<noun>` so the prefix invariant holds
/// regardless of where the page sits on disk.
pub(crate) fn resolve_page_namespace(
    page_abs: &Path,
    repo_root: &Path,
    wiki_infos: &[WikiInfo],
    fm_namespace: Option<&str>,
) -> PageNamespace {
    // Pick the longest wiki root that contains this page (deeper roots win).
    let owner = wiki_infos
        .iter()
        .filter(|w| page_abs.starts_with(&w.root))
        .max_by_key(|w| w.root.components().count());
    if let Some(w) = owner {
        let rel = page_abs.strip_prefix(&w.root).unwrap_or(page_abs);
        let parent = rel.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let subdir = parent.to_string_lossy().replace('\\', "/");
        return PageNamespace {
            namespace: w.namespace.clone(),
            subdir,
        };
    }
    // Float page: honor frontmatter `namespace`. Drop the file's directory
    // path — float pages don't carry a wiki-root path component, only the
    // declared namespace.
    let _ = repo_root; // kept in signature for symmetry; unused on the float path
    PageNamespace {
        namespace: fm_namespace.map(|s| s.to_string()),
        subdir: String::new(),
    }
}

/// Classify the frontmatter of a file, returning both the `FileMeta` and an
/// optional `ParseErrorKind` if the file's `title` could not be extracted.
fn classify_frontmatter(path: &Path, repo_root: &Path, source: DocSource) -> (FileMeta, Option<ParseErrorKind>) {
    let text = match read_via_source(path, repo_root, source) {
        Ok(s) => s,
        Err(e) => {
            return (
                FileMeta::default(),
                Some(ParseErrorKind::Unreadable(e.to_string())),
            );
        }
    };

    // Step 2: must start with `---\n` or `---\r\n`.
    if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
        return (FileMeta::default(), Some(ParseErrorKind::NoFrontmatter));
    }

    // Step 3: locate closing `---` fence.
    let after_open = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
        .unwrap_or(&text[4..]);
    let has_closing_fence = after_open
        .lines()
        .any(|l| l.trim_end_matches('\r') == "---");
    if !has_closing_fence {
        return (FileMeta::default(), Some(ParseErrorKind::Malformed));
    }

    // Step 4: look for `title:` line inside the fenced block.
    // Collect lines between the two `---` fences.
    let lines: Vec<&str> = after_open.lines().collect();
    let closing_idx = lines.iter().position(|l| l.trim_end_matches('\r') == "---");
    let fm_lines = match closing_idx {
        Some(i) => &lines[..i],
        None => &lines[..],
    };

    let title_line = fm_lines
        .iter()
        .find(|l| l.starts_with("title:") || l.starts_with("title :"));

    if title_line.is_none() {
        return (FileMeta::default(), Some(ParseErrorKind::MissingTitle));
    }

    // Check if the value is empty/whitespace.
    let raw_value = title_line
        .unwrap()
        .split_once(':')
        .map(|(_, v)| v)
        .unwrap_or("")
        .trim();
    let stripped_value = raw_value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            raw_value
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(raw_value)
        .trim();

    if stripped_value.is_empty() {
        return (FileMeta::default(), Some(ParseErrorKind::EmptyTitle));
    }

    // Step 5: run parse_frontmatter_field — if it returns None despite a
    // non-empty title line, the frontmatter is malformed (BOM, CRLF, etc.).
    let title = parse_frontmatter_field(&text, "title");
    if title.is_none() {
        return (FileMeta::default(), Some(ParseErrorKind::Malformed));
    }

    // `namespace` is optional and only meaningful for `.wiki.md` float pages;
    // pages under a `wiki.toml` inherit their namespace from the root and the
    // field is ignored. Read it eagerly so the resolver can use it later.
    let namespace = parse_frontmatter_field(&text, "namespace");

    let meta = FileMeta { title, namespace };
    (meta, None)
}

fn parse_frontmatter_field(content: &str, field: &str) -> Option<String> {
    // Only parse if the file starts with `---\n`. JS uses /^---\s*\n(?:.*\n)*?title:\s*(.+?)\s*\n/.
    // Anchor to file start (\A) so a thematic-break `---` later in the body does not
    // match — that was the JS prototype's intent.
    let pat = format!(r"\A---\s*\n(?:.*\n)*?{field}:\s*(.+?)\s*\n");
    let re = Regex::new(&pat).ok()?;
    let cap = re.captures(content)?;
    let raw = cap.get(1)?.as_str().trim();
    let stripped = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw);
    Some(stripped.trim().to_string())
}

fn path_relative_to(path: &Path, repo_root: &Path) -> String {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

fn locate_existing_suffix(rel_path: &str, repo_root: &Path) -> Option<String> {
    if repo_root.join(rel_path).exists() {
        return Some(rel_path.to_string());
    }
    let parts: Vec<&str> = rel_path.split('/').collect();
    for start in 1..parts.len() {
        let candidate = parts[start..].join("/");
        if candidate.is_empty() {
            continue;
        }
        if repo_root.join(&candidate).exists() {
            return Some(candidate);
        }
    }
    None
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_extracts_title_and_summary() {
        let c = "---\ntitle: Hello World\nsummary: A page summary.\n---\nbody";
        assert_eq!(
            parse_frontmatter_field(c, "title"),
            Some("Hello World".into())
        );
        assert_eq!(
            parse_frontmatter_field(c, "summary"),
            Some("A page summary.".into())
        );
    }

    #[test]
    fn parse_frontmatter_handles_quoted_values() {
        let c = "---\ntitle: \"Quoted Title\"\n---\n";
        assert_eq!(
            parse_frontmatter_field(c, "title"),
            Some("Quoted Title".into())
        );
    }

    #[test]
    fn parse_frontmatter_returns_none_when_absent() {
        assert!(parse_frontmatter_field("no frontmatter here", "title").is_none());
    }

    #[test]
    fn parse_frontmatter_ignores_thematic_break_in_body() {
        // Body contains a `---` separator followed by a `title:` line — must NOT match.
        let c = "# Heading\n\nbody text\n\n---\ntitle: Spurious\n\nmore body\n";
        assert_eq!(parse_frontmatter_field(c, "title"), None);
    }

    // ── heading-chain trim ────────────────────────────────────────────────────

    #[test]
    fn trim_chain_drops_leading_when_equals_page_title() {
        let chain = vec!["Billing".to_string(), "Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_chain_keeps_chain_when_top_differs() {
        let chain = vec!["Charge handler".to_string()];
        let trimmed = trim_heading_chain(&chain, "Billing");
        assert_eq!(trimmed, vec!["Charge handler"]);
    }

    #[test]
    fn trim_chain_empties_to_nothing_when_single_equals_title() {
        let chain = vec!["Incremental indexing".to_string()];
        let trimmed = trim_heading_chain(&chain, "Incremental indexing");
        assert!(trimmed.is_empty());
    }

    // ── classify_frontmatter unit tests ──────────────────────────────────────

    fn classify_str(text: &str) -> Option<ParseErrorKind> {
        // Write to a tempfile and run the classifier.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let dir = std::env::temp_dir();
        std::fs::write(tmp.path(), text.as_bytes()).unwrap();
        let (_, kind) = classify_frontmatter(tmp.path(), &dir, DocSource::WorkingTree);
        kind
    }

    #[test]
    fn classify_no_frontmatter() {
        let kind = classify_str("# Just a body.\n");
        assert!(
            matches!(kind, Some(ParseErrorKind::NoFrontmatter)),
            "expected NoFrontmatter, got {kind:?}"
        );
    }

    #[test]
    fn classify_missing_title() {
        let kind = classify_str("---\nsummary: x\n---\n\nbody\n");
        assert!(
            matches!(kind, Some(ParseErrorKind::MissingTitle)),
            "expected MissingTitle, got {kind:?}"
        );
    }

    #[test]
    fn classify_empty_title() {
        let kind = classify_str("---\ntitle:\nsummary: x\n---\n\nbody\n");
        assert!(
            matches!(kind, Some(ParseErrorKind::EmptyTitle)),
            "expected EmptyTitle, got {kind:?}"
        );
    }

    #[test]
    fn classify_malformed_bom() {
        // BOM-prefixed frontmatter — parse_frontmatter_field will return None.
        let kind = classify_str("\u{FEFF}---\ntitle: x\nsummary: y\n---\n");
        assert!(
            matches!(
                kind,
                Some(ParseErrorKind::Malformed | ParseErrorKind::NoFrontmatter)
            ),
            "expected Malformed or NoFrontmatter, got {kind:?}"
        );
    }

    #[test]
    fn classify_unreadable_non_utf8() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), [0xFF_u8, 0xFE, 0x00]).unwrap();
        let (_, kind) = classify_frontmatter(tmp.path(), &std::env::temp_dir(), DocSource::WorkingTree);
        assert!(
            matches!(kind, Some(ParseErrorKind::Unreadable(_))),
            "expected Unreadable, got {kind:?}"
        );
    }

    #[test]
    fn classify_clean_file() {
        let kind = classify_str("---\ntitle: Hello\nsummary: World\n---\n\nbody\n");
        assert!(kind.is_none(), "expected no error, got {kind:?}");
    }

    // ── resolve_slug_collisions ──────────────────────────────────────────────

    use super::super::draft::StructuredAnchor;

    fn make_draft(
        page_path: &str,
        noun: &str,
        page_ns: PageNamespace,
        heading_chain: Vec<&str>,
    ) -> MeshDraft {
        let slug = super::super::draft::build_slug(&page_ns, noun);
        MeshDraft {
            page_path: page_path.to_string(),
            slug,
            anchors: Vec::new(),
            structured_anchors: vec![StructuredAnchor {
                path: page_path.to_string(),
                start_line: 1,
                end_line: 1,
            }],
            heading_chain: heading_chain.iter().map(|s| s.to_string()).collect(),
            consolidated_count: 1,
            noun: noun.to_string(),
            page_ns,
        }
    }

    fn ns(namespace: Option<&str>, subdir: &str) -> PageNamespace {
        PageNamespace {
            namespace: namespace.map(|s| s.to_string()),
            subdir: subdir.to_string(),
        }
    }

    #[test]
    fn collision_resolver_keeps_unique_slugs() {
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            ns(None, ""),
            vec![],
        )];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let exists = |_slug: &str| false;
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/charge-handler");
    }

    #[test]
    fn collision_resolver_adds_parent_heading_on_clash() {
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            ns(None, ""),
            vec!["Checkout"],
        )];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let existing: std::collections::HashSet<String> = ["wiki/charge-handler".to_string()]
            .into_iter()
            .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/checkout/charge-handler");
    }

    #[test]
    fn collision_resolver_walks_chain_outer_to_inner() {
        // chain is outer→inner. Base slug clashes; +inner-most ("Checkout")
        // also clashes; the resolver must try adding the next ancestor up
        // ("Payments") before falling back further.
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            ns(None, ""),
            vec!["Payments", "Checkout"],
        )];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let existing: std::collections::HashSet<String> = [
            "wiki/charge-handler".to_string(),
            "wiki/checkout/charge-handler".to_string(),
        ]
        .into_iter()
        .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/payments/checkout/charge-handler");
    }

    #[test]
    fn collision_resolver_uses_page_title_after_chain_exhausted() {
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            ns(None, ""),
            vec!["Checkout"],
        )];
        let mut titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        titles.insert("wiki/billing.md".to_string(), Some("Billing Service".to_string()));
        let existing: std::collections::HashSet<String> = [
            "wiki/charge-handler".to_string(),
            "wiki/checkout/charge-handler".to_string(),
        ]
        .into_iter()
        .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(
            drafts[0].slug,
            "wiki/billing-service/checkout/charge-handler"
        );
    }

    #[test]
    fn collision_resolver_falls_back_to_digit_suffix() {
        let mut drafts = vec![make_draft(
            "wiki/billing.md",
            "charge-handler",
            ns(None, ""),
            vec![],
        )];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let existing: std::collections::HashSet<String> = ["wiki/charge-handler".to_string()]
            .into_iter()
            .collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        // No chain, no title — only the base slug is a unique candidate,
        // so the digit fallback runs against it.
        assert_eq!(drafts[0].slug, "wiki/charge-handler-2");
    }

    #[test]
    fn collision_resolver_dedups_within_run() {
        // Two drafts with the same base slug and no semantic disambiguators
        // available; the second must get a digit suffix.
        let mut drafts = vec![
            make_draft("wiki/a.md", "foo", ns(None, ""), vec![]),
            make_draft("wiki/b.md", "foo", ns(None, ""), vec![]),
        ];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let exists = |_slug: &str| false;
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/foo");
        assert_eq!(drafts[1].slug, "wiki/foo-2");
    }

    #[test]
    fn collision_resolver_intra_run_uses_semantic_qualifier_first() {
        let mut drafts = vec![
            make_draft("wiki/a.md", "foo", ns(None, ""), vec!["First"]),
            make_draft("wiki/b.md", "foo", ns(None, ""), vec!["Second"]),
        ];
        let titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        let exists = |_slug: &str| false;
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/foo");
        // Second clashes only with the first draft (in-run); the parent
        // heading "Second" resolves it without needing a digit.
        assert_eq!(drafts[1].slug, "wiki/second/foo");
    }

    #[test]
    fn collision_resolver_skips_duplicate_candidates_from_reserved_drop() {
        // Page lives in namespace "mesh" at root; parent heading is also "mesh",
        // so the +parent candidate collapses back to the base slug. The resolver
        // must skip that duplicate and try the next strategy (page title).
        let mut drafts = vec![make_draft(
            "mesh/page.md",
            "leaf",
            ns(Some("mesh"), ""),
            vec!["Mesh"],
        )];
        let mut titles: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        titles.insert("mesh/page.md".to_string(), Some("Outer".to_string()));
        let existing: std::collections::HashSet<String> =
            ["wiki/mesh/leaf".to_string()].into_iter().collect();
        let exists = |slug: &str| existing.contains(slug);
        resolve_slug_collisions(&mut drafts, &titles, &exists);
        assert_eq!(drafts[0].slug, "wiki/mesh/outer/leaf");
    }
}
