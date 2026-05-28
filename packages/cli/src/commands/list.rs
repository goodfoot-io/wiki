use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use miette::{IntoDiagnostic, Result};
use serde::Serialize;

use crate::frontmatter::parse_frontmatter;
use crate::index::DocSource;

#[derive(Debug, Serialize)]
pub struct PageEntry {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub summary: String,
    pub file: String,
}

pub fn run(
    _globs: &[String],
    tag: Option<&str>,
    limit: Option<u64>,
    offset: Option<u64>,
    json: bool,
    repo_root: &Path,
    _source: DocSource,
) -> Result<i32> {
    let tag_filter = tag.map(|t| t.to_lowercase());
    let offset = offset.unwrap_or(0);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut skipped: u64 = 0;
    let mut emitted: u64 = 0;
    let mut first = true;

    if json {
        write!(out, "[").into_diagnostic()?;
    }

    let walker = ignore::WalkBuilder::new(repo_root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build();

    for entry in walker {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        // Skip the .git directory and its contents.
        if let Ok(rel) = path.strip_prefix(repo_root)
            && rel.components().next().map(|c| c.as_os_str()) == Some(OsStr::new(".git"))
        {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let fm = match parse_frontmatter(&content, path) {
            Ok(Some(fm)) => fm,
            _ => continue,
        };

        if let Some(ref tag_lc) = tag_filter
            && !fm.tags.iter().any(|t| t.to_lowercase() == *tag_lc)
        {
            continue;
        }

        if skipped < offset {
            skipped += 1;
            continue;
        }

        let rel = path.strip_prefix(repo_root).unwrap_or(path);
        let page = PageEntry {
            title: fm.title,
            aliases: fm.aliases,
            tags: fm.tags,
            summary: fm.summary,
            file: rel.to_string_lossy().into_owned(),
        };

        if json {
            if !first {
                write!(out, ",").into_diagnostic()?;
            }
            let s = serde_json::to_string(&page).into_diagnostic()?;
            out.write_all(s.as_bytes()).into_diagnostic()?;
            first = false;
        } else {
            write_markdown(&mut out, &page).into_diagnostic()?;
        }

        emitted += 1;
        if let Some(n) = limit
            && emitted >= n
        {
            break;
        }
    }

    if json {
        writeln!(out, "]").into_diagnostic()?;
    }

    Ok(0)
}

fn write_markdown<W: Write>(out: &mut W, entry: &PageEntry) -> io::Result<()> {
    writeln!(out, "**{}** — `{}`", entry.title, entry.file)?;
    let mut meta = Vec::new();
    if !entry.aliases.is_empty() {
        meta.push(format!(
            "aliases: {}",
            entry
                .aliases
                .iter()
                .map(|alias| format!("`{alias}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !entry.tags.is_empty() {
        meta.push(format!(
            "tags: {}",
            entry
                .tags
                .iter()
                .map(|tag| format!("`{tag}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !meta.is_empty() {
        writeln!(out, "{}", meta.join(" · "))?;
    }
    writeln!(out, "\n{}\n\n---\n", entry.summary)?;
    Ok(())
}
