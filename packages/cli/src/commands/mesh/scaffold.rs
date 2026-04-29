//! `wiki mesh scaffold` end-to-end pipeline.
//!
//! Discover wiki files, parse their fragment links, and emit a shell script
//! (or JSON) of `git mesh add` / `git mesh why` commands — one mesh per link.
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
use super::name::{
    co_presence_terms, deduplicate_names, detect_category, detect_rel_type, extract_source_role,
    extract_target_role, norm_cmp, rake, select_core_phrase, slugify, tokenize,
};
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

/// Run the `wiki mesh scaffold` subcommand.
pub fn run(globs: &[String], json: bool, repo_root: &Path) -> Result<i32> {
    let files = discover_files(globs, repo_root)?;

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
        // Match the JS: header (shell) or `[]` (json) and exit 0.
        if json {
            println!("[]");
        } else {
            print!("{SHELL_HEADER}");
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

    // Shell mode: header + per-wiki-file groups.
    println!("{SHELL_HEADER}");
    // Group by source wiki file, preserving the order in which each wiki file
    // first appeared in `discover_files`. Lex-sorted output (BTreeMap) would
    // match the current fixture by accident but break diffs the moment a new
    // file is added mid-alphabet.
    let mut by_file: Vec<(String, Vec<&Mesh>)> = Vec::new();
    for m in &meshes {
        if let Some(entry) = by_file.iter_mut().find(|(k, _)| *k == m.wiki_file) {
            entry.1.push(m);
        } else {
            by_file.push((m.wiki_file.clone(), vec![m]));
        }
    }
    for (wiki_file, entries) in by_file {
        let pad = 60usize.saturating_sub(wiki_file.len());
        let dashes: String = "─".repeat(pad);
        println!("# ── {wiki_file} {dashes}");
        for m in entries {
            println!();
            println!("git mesh add {} \\", m.name);
            println!("  {} \\", m.wiki_file);
            println!("  {}", m.anchor);
            println!(
                "git mesh why {} -m \"{}\"",
                m.name,
                shell_double_quote_escape(&m.why)
            );
        }
        println!();
    }
    Ok(0)
}

const SHELL_HEADER: &str = "#!/bin/sh\n\
# Generated by wiki mesh scaffold\n\
# Review names and whys before running. Commit: git mesh commit <name>\n";

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

    let wiki_file_rel = path_relative_to(&input.wiki_file, repo_root);
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

/// Escape a string for safe interpolation inside shell double-quotes.
///
/// Inside `"..."` bash still interprets `\`, `` ` ``, `$`, and `"` — a backtick
/// in prose triggers command substitution at run time. We escape backslash
/// first so subsequent escape characters we add aren't themselves doubled.
fn shell_double_quote_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '`' => out.push_str(r"\`"),
            '$' => out.push_str(r"\$"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
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
    fn shell_double_quote_escape_handles_special_chars() {
        // Verify all four shell-special characters are escaped (backslash first).
        assert_eq!(shell_double_quote_escape(r#"a"b`c$d\e"#), r#"a\"b\`c\$d\\e"#);
        // Plain text round-trips unchanged.
        assert_eq!(
            shell_double_quote_escape("just some prose."),
            "just some prose."
        );
    }

    #[test]
    fn strip_bare_line_range_replaces_with_empty() {
        assert_eq!(strip_bare_line_range("L19-L22"), "");
        assert_eq!(strip_bare_line_range("L5"), "");
        assert_eq!(strip_bare_line_range("hello"), "hello");
    }
}
