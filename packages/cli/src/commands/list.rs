use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::index::{PageListEntry, WikiIndex};

#[derive(Debug, Serialize)]
pub struct PageEntry {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub summary: String,
    pub file: String,
}

pub fn run(_globs: &[String], tag: Option<&str>, json: bool, wiki_root: &Path, repo_root: &Path) -> Result<i32> {
    let index = WikiIndex::prepare(wiki_root, repo_root)?;
    let entries = index
        .list_pages(tag)?
        .into_iter()
        .map(page_entry)
        .collect::<Vec<_>>();

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

fn page_entry(entry: PageListEntry) -> PageEntry {
    PageEntry {
        title: entry.title,
        aliases: entry.aliases,
        tags: entry.tags,
        summary: entry.summary,
        file: entry.file,
    }
}
