use std::io::{self, Write};
use std::path::Path;

use miette::{IntoDiagnostic, Result};
use serde::Serialize;

use crate::index::{DocSource, WikiIndex};

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
    source: DocSource,
) -> Result<i32> {
    let offset = offset.unwrap_or(0);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut first = true;

    if json {
        write!(out, "[").into_diagnostic()?;
    }

    let index = WikiIndex::prepare_for_source(repo_root, source)?;
    let rows = index.list_pages(tag, offset, limit)?;

    for row in rows {
        let page = PageEntry {
            title: row.title,
            aliases: row.aliases,
            tags: row.tags,
            summary: row.summary,
            file: row.path_rel,
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
