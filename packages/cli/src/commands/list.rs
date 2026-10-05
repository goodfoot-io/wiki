use std::io::{self, Write};
use std::path::Path;

use miette::{IntoDiagnostic, Result};
use serde::Serialize;

use crate::index::{DocSource, WikiIndex};
use crate::output::{Stdout, StdoutResultExt as _};

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
    let mut first = true;

    // The opening bracket is written before the index is prepared and waits
    // in stdout's line buffer; the lock is not held across the preparation.
    if json {
        write!(Stdout::lock(), "[")?;
    }

    let index = WikiIndex::prepare_for_source(repo_root, source)?;
    let rows = index.list_pages(tag, offset, limit)?;

    let mut out = Stdout::lock();
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
                write!(out, ",")?;
            }
            let s = serde_json::to_string(&page).into_diagnostic()?;
            write!(out, "{s}")?;
            first = false;
        } else {
            write_markdown(out.raw(), &page).on_stdout()?;
        }
    }

    if json {
        writeln!(out, "]")?;
    }
    out.flush()?;

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
