//! `wiki scaffold` end-to-end pipeline.
//!
//! Discover wiki files, parse their fragment links, and emit a markdown
//! document (or JSON) of `git mesh add` / `git mesh why` commands — one mesh
//! per link.
//!
//! Phase B: name generation is fully wired; whys are template-only because
//! [`why::extract_prose_why`] is a Phase B stub returning `None`. Phase C will
//! add the prose-why heuristics.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use miette::{IntoDiagnostic, Result, WrapErr};
use regex::Regex;
use serde::Serialize;

use crate::commands::{discover_files, resolve_link_path};
use crate::parser::{LinkKind, parse_fragment_links};

use super::augment::{AugmentedLink, augment};
use super::draft::{self, MeshDraft};
use super::group;
use super::name::{
    co_presence_terms, deduplicate_names, detect_category, detect_rel_type, extract_source_role,
    extract_target_role, norm_cmp, rake, select_core_phrase, slugify, tokenize,
};
use super::render;
use super::why::{extract_prose_why, template_why};

/// Output of mesh generation for a single fragment link.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Mesh {
    pub(crate) name: String,
    pub(crate) why: String,
    #[serde(rename = "wikiFile")]
    pub(crate) wiki_file: String,
    pub(crate) anchor: String,
}

/// Run the `wiki scaffold` subcommand.
pub fn run(globs: &[String], json: bool, wiki_root: &Path, repo_root: &Path) -> Result<i32> {
    let discovered = discover_files(globs, wiki_root, repo_root)?;
    // Filter test fixtures: `wiki check` legitimately scans them, but a
    // scaffold run that materializes mesh commands for a test wiki would
    // pollute the repo's mesh state on the first commit. Scoped to scaffold.
    let files: Vec<PathBuf> = discovered
        .into_iter()
        .filter(|p| {
            let s = p.to_string_lossy();
            !s.contains("/tests/fixtures/") && !s.contains("\\tests\\fixtures\\")
        })
        .collect();

    let mut all_inputs: Vec<LinkInput> = Vec::new();
    for file in &files {
        let content = fs::read_to_string(file)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to read wiki file: {}", file.display()))?;
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

    if all_inputs.is_empty() {
        if json {
            println!("[]");
        }
        return Ok(0);
    }

    // Build per-source frontmatter map (title, summary) keyed by absolute path.
    let mut meshes: Vec<Mesh> = Vec::with_capacity(all_inputs.len());
    let mut wiki_meta_cache: std::collections::HashMap<PathBuf, FileMeta> =
        std::collections::HashMap::new();
    for f in &files {
        let meta = read_file_meta(f);
        wiki_meta_cache.insert(f.clone(), meta);
    }
    let mut target_meta_cache: std::collections::HashMap<PathBuf, FileMeta> =
        std::collections::HashMap::new();

    for input in &all_inputs {
        let src_meta = wiki_meta_cache
            .get(&input.wiki_file)
            .cloned()
            .unwrap_or_default();

        // Resolve target path against the source file then against repo root.
        let resolved = resolve_link_path(&input.augmented.link.path, &input.wiki_file, repo_root);
        let target_abs = if resolved.is_absolute() {
            resolved.clone()
        } else {
            repo_root.join(&resolved)
        };
        let tgt_meta = target_meta_cache
            .entry(target_abs.clone())
            .or_insert_with(|| read_file_meta(&target_abs))
            .clone();

        let mesh = generate_mesh(input, &src_meta, &tgt_meta, repo_root);
        meshes.push(mesh);
    }

    // Global dedup — match JS in-place rename.
    let mut names: Vec<String> = meshes.iter().map(|m| m.name.clone()).collect();
    deduplicate_names(&mut names);
    for (m, n) in meshes.iter_mut().zip(names) {
        m.name = n;
    }

    if json {
        let s = serde_json::to_string_pretty(&meshes).into_diagnostic()?;
        println!("{s}");
        return Ok(0);
    }

    // ── Markdown mode: build/group pipeline ──────────────────────────────
    let drafts_by_page = build_meshes(&all_inputs, repo_root);

    // Build the page-title lookup keyed by the same repo-root-relative
    // `page_path` strings the drafts use, so the renderer can prefix each
    // per-page section with the source page's frontmatter `title`.
    let mut page_titles: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    for f in &files {
        let rel = path_relative_to(f, repo_root);
        let title = wiki_meta_cache.get(f).and_then(|m| m.title.clone());
        page_titles.insert(rel, title);
    }

    let rendered = render::render_markdown(&drafts_by_page, &page_titles);
    print!("{rendered}");
    Ok(0)
}

/// Three-stage build/group/annotate pipeline that produces the final list of
/// meshes (in per-page declaration order) ready for shell rendering.
fn build_meshes(inputs: &[LinkInput], repo_root: &Path) -> Vec<MeshDraft> {
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

    // Stage 1: build drafts per page.
    let mut all_drafts: Vec<MeshDraft> = Vec::new();
    let mut page_spans: Vec<(usize, usize)> = Vec::with_capacity(page_order.len());
    for page in &page_order {
        let entries = by_page.get(page).expect("page tracked in order");
        let page_rel = path_relative_to(page, repo_root);
        let augs: Vec<AugmentedLink> = entries.iter().map(|i| i.augmented.clone()).collect();
        let target_anchors: Vec<Vec<String>> = entries
            .iter()
            .map(|i| {
                let link = &i.augmented.link;
                let resolved = resolve_link_path(&link.path, &i.wiki_file, repo_root);
                let anchor_rel = path_relative_to(&resolved, repo_root);
                let anchor_rel =
                    locate_existing_suffix(&anchor_rel, repo_root).unwrap_or(anchor_rel);
                let start = link.start_line.unwrap_or(0);
                let end = link.end_line.unwrap_or(start);
                vec![format!("{anchor_rel}#L{start}-L{end}")]
            })
            .collect();
        let drafts = draft::build(&page_rel, &augs, &target_anchors, repo_root);
        let start = all_drafts.len();
        all_drafts.extend(drafts);
        page_spans.push((start, all_drafts.len()));
    }

    // Stage 2a: per-page consolidation FIRST. Identical-anchor-set siblings
    // collapse into one survivor; only then does the global dedup pass see
    // contiguous slug counts. Doing this in the reverse order leaks suffix
    // gaps (`foo`, `foo-3`, no `foo-2`) into the footer's `git mesh commit`
    // lines whenever consolidation prunes a duplicate the dedup already
    // suffixed.
    let mut consolidated: Vec<MeshDraft> = Vec::new();
    for (start, end) in page_spans {
        let page_drafts: Vec<MeshDraft> = all_drafts[start..end].to_vec();
        consolidated.extend(group::consolidate_within_page(page_drafts));
    }

    // Stage 2b: global slug dedup over the merge survivors.
    dedup_slugs(&mut consolidated);

    consolidated
}

/// First occurrence keeps the original slug; subsequent duplicates get
/// `-2`, `-3`, … suffixes.
fn dedup_slugs(drafts: &mut [MeshDraft]) {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for d in drafts.iter_mut() {
        let count = seen.entry(d.slug.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            d.slug = format!("{}-{}", d.slug, *count);
        }
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
    summary: Option<String>,
    content: Option<String>,
}

fn read_file_meta(path: &Path) -> FileMeta {
    let content = fs::read_to_string(path).ok();
    let mut meta = FileMeta {
        content: content.clone(),
        ..Default::default()
    };
    if let Some(text) = &content {
        meta.title = parse_frontmatter_field(text, "title");
        meta.summary = parse_frontmatter_field(text, "summary");
    }
    meta
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

fn generate_mesh(
    input: &LinkInput,
    src_meta: &FileMeta,
    tgt_meta: &FileMeta,
    repo_root: &Path,
) -> Mesh {
    let link = &input.augmented.link;
    let surrounding = &input.augmented.surrounding_text;
    let heading_chain = &input.augmented.heading_chain;

    let source_title_tokens: BTreeSet<String> = tokenize(src_meta.title.as_deref().unwrap_or(""))
        .into_iter()
        .collect();

    let start_line = link.start_line.unwrap_or(0);
    let end_line = link.end_line.unwrap_or(start_line);

    // Compute wiki section line range based on the link's source line.
    let wiki_section_range = src_meta
        .content
        .as_ref()
        .and_then(|content| find_wiki_section_range_for_link(content, link.source_line));

    // Target snippet: lines [startLine-1 .. endLine+5] of target content (1-based).
    let target_snippet = tgt_meta.content.as_ref().map(|c| {
        let lines: Vec<&str> = c.lines().collect();
        let s = (start_line as usize).saturating_sub(1);
        let e = std::cmp::min(lines.len(), end_line as usize + 5);
        lines[s.min(lines.len())..e].join("\n")
    });

    // sourceCtx and targetCtx — match JS join-with-space.
    let mut source_ctx = surrounding.clone();
    for h in heading_chain {
        source_ctx.push(' ');
        source_ctx.push_str(h);
    }

    let mut target_ctx = String::new();
    if let Some(snip) = &target_snippet {
        target_ctx.push_str(snip);
    }
    if let Some(t) = &tgt_meta.title {
        if !target_ctx.is_empty() {
            target_ctx.push(' ');
        }
        target_ctx.push_str(t);
    }
    if let Some(s) = &tgt_meta.summary {
        if !target_ctx.is_empty() {
            target_ctx.push(' ');
        }
        target_ctx.push_str(s);
    }

    let rake_results = rake(surrounding);
    let co_present = co_presence_terms(&source_ctx, &target_ctx);

    // Strip backtick/bold/etc. + bare line-range labels + adjacent dup words.
    let mut link_text = strip_link_text_ornaments(&link.original_text);
    link_text = strip_bare_line_range(&link_text);
    link_text = strip_adjacent_duplicates(&link_text);

    let core_phrase = select_core_phrase(
        &rake_results,
        &co_present,
        &link_text,
        &link.path,
        &source_title_tokens,
    );

    let combined_ctx = format!("{source_ctx} {target_ctx}");
    let all_tokens = tokenize(&combined_ctx);

    let mut wiki_file_rel = path_relative_to(&input.wiki_file, repo_root);
    // Append wiki section range if available.
    if let Some((section_start, section_end)) = wiki_section_range {
        wiki_file_rel = format!("{wiki_file_rel}#L{section_start}-L{section_end}");
    }
    let path_input = format!("{wiki_file_rel} {}", link.path);
    let path_tokens = tokenize(&path_input);

    let mut heading_input = String::new();
    if let Some(t) = &src_meta.title {
        heading_input.push_str(t);
    }
    for h in heading_chain {
        heading_input.push(' ');
        heading_input.push_str(h);
    }
    let heading_tokens = tokenize(&heading_input);

    let rel_type = detect_rel_type(&all_tokens);
    let category = detect_category(&all_tokens, &path_tokens, &heading_tokens);

    let target_role = extract_target_role(&link.path, tgt_meta.title.as_deref());
    let source_role = extract_source_role(heading_chain, src_meta.title.as_deref());

    let object_phrase =
        compute_object_phrase(&co_present, tgt_meta, &source_title_tokens, &target_role);

    let prose_why = extract_prose_why(&input.augmented);

    // Bare line-range corePhrase: substitute objectPhrase or targetRole.
    let line_range_re = Regex::new(r"^(?i)L\d+(-L?\d+)?$").expect("valid regex");
    let effective_core = if line_range_re.is_match(core_phrase.trim()) {
        if !object_phrase.is_empty() && norm_cmp(&object_phrase) != norm_cmp(&target_role) {
            object_phrase.clone()
        } else {
            target_role.clone()
        }
    } else {
        core_phrase.clone()
    };

    let why = prose_why.unwrap_or_else(|| {
        template_why(
            rel_type,
            &effective_core,
            &object_phrase,
            &source_role,
            &target_role,
        )
    });

    // Name: wiki/<category>/<coreSlug>
    let core_slug = {
        let slug = slugify(&effective_core);
        let line_slug_re = Regex::new(r"^(?i)l\d+-l\d+$").expect("valid regex");
        if (category.is_some() && Some(slug.as_str()) == category)
            || line_slug_re.is_match(&slug)
            || slug == "relationship"
        {
            let alt = slugify(&target_role);
            if alt.is_empty() { slug } else { alt }
        } else {
            slug
        }
    };
    let name = match category {
        Some(c) => format!("wiki/{c}/{core_slug}"),
        None => format!("wiki/{core_slug}"),
    };

    // Resolve the link path against the source wiki file, then express it
    // repo-root-relative with forward slashes. The generated `git mesh add`
    // commands run from the repo root, so a `./bar.rs` link from
    // `wiki/foo.md` must become `wiki/bar.rs` (or wherever it actually lives).
    let anchor_resolved = resolve_link_path(&link.path, &input.wiki_file, repo_root);
    let anchor_rel = path_relative_to(&anchor_resolved, repo_root);
    // If the literal path doesn't exist in the repo, peel leading components
    // until we find one that does — this lets wiki pages copied from another
    // repo retain links that originally encoded a foreign repo's path prefix.
    let anchor_rel = locate_existing_suffix(&anchor_rel, repo_root).unwrap_or(anchor_rel);
    let anchor = format!("{anchor_rel}#L{start_line}-L{end_line}");

    Mesh {
        name,
        why,
        wiki_file: wiki_file_rel,
        anchor,
    }
}

fn compute_object_phrase(
    co_present: &[super::name::CoPresentTerm],
    tgt_meta: &FileMeta,
    source_title_tokens: &BTreeSet<String>,
    target_role: &str,
) -> String {
    if !co_present.is_empty()
        && let Some(c) = co_present
            .iter()
            .find(|x| !source_title_tokens.contains(&x.term))
    {
        return c.term.clone();
    }
    if let Some(t) = &tgt_meta.title {
        let toks: Vec<String> = tokenize(t)
            .into_iter()
            .filter(|w| {
                super::words::STOP.binary_search(&w.as_str()).is_err()
                    && super::words::NOISE.binary_search(&w.as_str()).is_err()
            })
            .collect();
        if !toks.is_empty() {
            return toks.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
        }
    }
    target_role.to_string()
}

fn strip_link_text_ornaments(s: &str) -> String {
    // /[`*_[\]]/g → ''
    let re = Regex::new(r"[`*_\[\]]").expect("valid regex");
    re.replace_all(s, "").trim().to_string()
}

fn strip_bare_line_range(s: &str) -> String {
    let re = Regex::new(r"^(?i)L\d+(-L?\d+)?$").expect("valid regex");
    if re.is_match(s) {
        String::new()
    } else {
        s.to_string()
    }
}

fn strip_adjacent_duplicates(s: &str) -> String {
    // Collapse runs of identical whitespace-separated words (case-insensitive).
    // Mirrors the JS `\b(\w+)(\s+\1)+\b → "$1"` — `regex` crate does not support
    // backreferences, so we walk tokens manually.
    let mut out: Vec<&str> = Vec::new();
    for tok in s.split_whitespace() {
        if let Some(prev) = out.last()
            && prev.eq_ignore_ascii_case(tok)
        {
            continue;
        }
        out.push(tok);
    }
    out.join(" ")
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

/// Find the line range of the wiki section containing the given link source line.
///
/// Finds the nearest heading at or before the link's source line, then determines
/// the extent of that section (up to the next heading at the same or higher level,
/// or EOF). Returns the (start_line, end_line) of that section (1-based).
fn find_wiki_section_range_for_link(content: &str, link_source_line: usize) -> Option<(u32, u32)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || link_source_line == 0 {
        return None;
    }

    // Find all headings in the document.
    let mut headings: Vec<(usize, usize)> = Vec::new(); // (line_num, level)
    for (i, line) in lines.iter().enumerate() {
        if let Some(level) = parse_heading_level(line) {
            headings.push((i + 1, level));
        }
    }

    // Find the heading that most closely precedes the link.
    let section_start = headings
        .iter()
        .filter(|(line_num, _)| *line_num <= link_source_line)
        .max_by_key(|(line_num, _)| *line_num)
        .map(|(line_num, _)| *line_num);

    match section_start {
        None => {
            // No heading before the link - section starts at document top.
            let section_end = headings.first().map(|(l, _)| l - 1).unwrap_or(lines.len());
            Some((1, section_end as u32))
        }
        Some(start_line) => {
            let (_, start_level) = headings.iter().find(|(l, _)| *l == start_line).unwrap();

            // Find the next heading at the same or higher level.
            let end_line = headings
                .iter()
                .find(|(l, level)| *l > start_line && *level <= *start_level)
                .map(|(l, _)| l - 1)
                .unwrap_or(lines.len());

            Some((start_line as u32, end_line as u32))
        }
    }
}

/// Find the line range of the wiki section containing the given link.
///
/// Uses the heading chain to identify which section the link belongs to,
/// then returns the (start_line, end_line) of that section (1-based).
/// Returns None if the section cannot be determined.
#[allow(dead_code)]
fn find_wiki_section_range(
    content: &str,
    heading_chain: &[String],
    _link_source_line: usize,
) -> Option<(u32, u32)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }

    // Parse heading level and text from each line.
    let mut headings: Vec<(usize, usize, usize)> = Vec::new(); // (line_num, level, heading_idx)
    for (i, line) in lines.iter().enumerate() {
        if let Some(level) = parse_heading_level(line) {
            headings.push((i + 1, level, heading_chain.len().saturating_sub(1))); // placeholder heading_idx
        }
    }

    // If the heading chain is empty, we're at the document top level.
    // Find section from start to first heading, or to EOF if no headings exist.
    if heading_chain.is_empty() {
        let end = headings
            .first()
            .map(|(l, _, _)| l - 1)
            .unwrap_or(lines.len());
        return Some((1, end.max(1) as u32));
    }

    // Match heading chain to actual headings in the document.
    // Start from line 1 and find headings matching the chain.
    let mut current_level = 0; // Track the heading level of the last match.
    let mut section_start_line = 1;
    let format_re = Regex::new(r"[`*_~\[\]]").expect("valid regex");

    for (chain_idx, chain_heading) in heading_chain.iter().enumerate() {
        let target_level = chain_idx + 1; // h1 is level 1, h2 is level 2, etc.
        // Strip markdown formatting from the chain heading for comparison.
        let normalized_chain_heading = format_re.replace_all(chain_heading, "").trim().to_string();

        // Find the next heading matching this level and text, starting from section_start_line.
        let mut found = false;
        for (line_num, level, _) in &headings {
            if *line_num < section_start_line {
                continue; // Skip headings before our current position.
            }
            if *level != target_level {
                continue; // Skip headings at other levels.
            }
            let heading_text = extract_heading_text(lines[line_num - 1]);
            if heading_text.eq_ignore_ascii_case(&normalized_chain_heading) {
                section_start_line = *line_num;
                current_level = *level;
                found = true;
                break;
            }
        }
        if !found {
            // If full chain doesn't match, try matching just from this heading onwards (fallback).
            // This handles cases where the heading chain might be incomplete or structured differently.
            if chain_idx == heading_chain.len() - 1 {
                // Last heading in chain - try to find ANY heading with this text
                for (line_num, level, _) in &headings {
                    let heading_text = extract_heading_text(lines[line_num - 1]);
                    if heading_text.eq_ignore_ascii_case(&normalized_chain_heading) {
                        section_start_line = *line_num;
                        current_level = *level;
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return None; // Heading chain doesn't match document structure.
            }
        }
    }

    // Now find the end of the section: the next heading at the same or higher level,
    // or EOF.
    let end_line = headings
        .iter()
        .find(|(line_num, level, _)| *line_num > section_start_line && *level <= current_level)
        .map(|(l, _, _)| l - 1)
        .unwrap_or(lines.len());

    Some((
        section_start_line as u32,
        end_line.max(section_start_line) as u32,
    ))
}

/// Extract the heading level (1-6) from a markdown line, or None if not a heading.
fn parse_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let mut level = 0;
    for c in trimmed.chars() {
        if c == '#' {
            level += 1;
        } else if c == ' ' {
            break;
        } else {
            return None; // Not a valid heading.
        }
    }
    if level > 0 && level <= 6 && trimmed.len() > level {
        Some(level)
    } else {
        None
    }
}

/// Extract the text content of a markdown heading (without the # symbols, whitespace, and markdown formatting).
#[allow(dead_code)]
fn extract_heading_text(line: &str) -> String {
    let trimmed = line.trim_start();
    let text = trimmed
        .trim_start_matches('#')
        .trim_start()
        .trim_end()
        .to_string();
    // Remove markdown formatting: backticks, bold, italic, etc.
    let re = Regex::new(r"[`*_~\[\]]").expect("valid regex");
    re.replace_all(&text, "").trim().to_string()
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
    fn strip_adjacent_duplicates_collapses() {
        assert_eq!(
            strip_adjacent_duplicates("extension extension"),
            "extension"
        );
        assert_eq!(strip_adjacent_duplicates("foo bar baz"), "foo bar baz");
    }

    #[test]
    fn parse_frontmatter_ignores_thematic_break_in_body() {
        // Body contains a `---` separator followed by a `title:` line — must NOT match.
        let c = "# Heading\n\nbody text\n\n---\ntitle: Spurious\n\nmore body\n";
        assert_eq!(parse_frontmatter_field(c, "title"), None);
    }

    #[test]
    fn strip_bare_line_range_replaces_with_empty() {
        assert_eq!(strip_bare_line_range("L19-L22"), "");
        assert_eq!(strip_bare_line_range("L5"), "");
        assert_eq!(strip_bare_line_range("hello"), "hello");
    }

    #[test]
    fn parse_heading_level_extracts_correctly() {
        assert_eq!(parse_heading_level("# Heading"), Some(1));
        assert_eq!(parse_heading_level("## Subheading"), Some(2));
        assert_eq!(parse_heading_level("### `Code Heading`"), Some(3));
        assert_eq!(parse_heading_level("#### Level 4"), Some(4));
        assert_eq!(parse_heading_level("Not a heading"), None);
        assert_eq!(parse_heading_level("#NoSpace"), None);
    }

    #[test]
    fn extract_heading_text_strips_formatting() {
        assert_eq!(extract_heading_text("# My Heading"), "My Heading");
        assert_eq!(extract_heading_text("## `Code Title`"), "Code Title");
        assert_eq!(
            extract_heading_text("### **Bold** and *italic*"),
            "Bold and italic"
        );
        assert_eq!(
            extract_heading_text("### `git-mesh ls <anchor> --porcelain`"),
            "git-mesh ls <anchor> --porcelain"
        );
    }

    #[test]
    fn find_wiki_section_range_simple() {
        let content = "# Heading 1\nParagraph 1\n## Heading 2\nParagraph 2\n## Heading 3\n";
        let chain = vec!["Heading 1".to_string(), "Heading 2".to_string()];
        let range = find_wiki_section_range(content, &chain, 3);
        // Section should start at "## Heading 2" (line 3) and end before "## Heading 3" (line 5)
        assert_eq!(range, Some((3, 4)));
    }

    #[test]
    fn find_wiki_section_range_with_backticks() {
        let content = "# Main\n## `Code Section`\nContent\n## Other\n";
        let chain = vec!["Main".to_string(), "Code Section".to_string()];
        let range = find_wiki_section_range(content, &chain, 3);
        // Section should be from line 2 ("## `Code Section`") to line 3 (before "## Other")
        assert_eq!(range, Some((2, 3)));
    }

    #[test]
    fn find_wiki_section_range_git_mesh_usage() {
        // Content with proper line structure:
        // 1: # Title
        // 2: ## Section
        // 3: ### Subsection
        // 4: Content
        // 5: ## Next Section
        let content = "# Title\n## Section\n### Subsection\nContent\n## Next Section\n";
        let chain = vec![
            "Title".to_string(),
            "Section".to_string(),
            "Subsection".to_string(),
        ];
        let range = find_wiki_section_range(content, &chain, 3);
        // Section should start at line 3 ("### Subsection") and end before line 5 ("## Next Section")
        // So the range should be (3, 4)
        assert_eq!(range, Some((3, 4)));
    }
}
