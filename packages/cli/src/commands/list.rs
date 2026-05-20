use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::index::{DocSource, PageListEntry, WikiIndex};

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
    let index = WikiIndex::prepare_for_source(repo_root, source)?;
    let all = index
        .list_pages(tag)?
        .into_iter()
        .map(page_entry)
        .collect::<Vec<_>>();

    let entries = paginate(all, offset, limit);

    if json {
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        for entry in &entries {
            println!("**{}** — `{}`", entry.title, entry.file);
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
                println!("{}", meta.join(" · "));
            }
            println!("\n{}\n\n---\n", entry.summary);
        }
    }

    Ok(0)
}

fn paginate(entries: Vec<PageEntry>, offset: Option<u64>, limit: Option<u64>) -> Vec<PageEntry> {
    let total = entries.len();
    let skip = offset.unwrap_or(0).min(total as u64) as usize;
    let remaining = total - skip;
    let take = match limit {
        Some(n) => (n as usize).min(remaining),
        None => remaining,
    };
    entries.into_iter().skip(skip).take(take).collect()
}

fn page_entry(entry: PageListEntry) -> PageEntry {
    PageEntry {
        title: entry.title,
        aliases: entry.aliases,
        tags: entry.tags,
        summary: entry.summary,
        file: entry.file,
    }
}
