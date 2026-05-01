use std::collections::HashSet;
use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::index::{DocSource, WikiIndex};
use crate::parser::parse_wikilinks;

#[derive(Debug, Serialize)]
pub struct ExtractEntry {
    pub title: String,
    pub summary: String,
    pub file: String,
}

pub fn run(input: &str, json: bool, wiki_root: &Path, repo_root: &Path, source: DocSource) -> Result<i32> {
    let wikilinks = parse_wikilinks(input);
    let mut seen = HashSet::new();
    let mut titles = Vec::new();
    for wikilink in wikilinks {
        let key = wikilink.title.to_lowercase();
        if seen.insert(key) {
            titles.push(wikilink.title);
        }
    }

    if titles.is_empty() {
        if json {
            println!("[]");
        }
        return Ok(0);
    }

    let index = WikiIndex::prepare_for_source(wiki_root, repo_root, source)?;
    let (resolved, unresolved) = index.extract_pages(&titles)?;

    let entries = resolved
        .into_iter()
        .map(|page| ExtractEntry {
            title: page.title,
            summary: page.summary,
            file: page.file,
        })
        .collect::<Vec<_>>();

    for title in &unresolved {
        eprintln!("No page found with title or alias `{title}`.");
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        for entry in &entries {
            println!("**{}** — {}", entry.title, entry.summary);
        }
    }

    Ok(if unresolved.is_empty() { 0 } else { 1 })
}
