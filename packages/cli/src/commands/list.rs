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

pub fn run(_globs: &[String], tag: Option<&str>, json: bool, wiki_root: &Path, repo_root: &Path, source: DocSource) -> Result<i32> {
    let index = WikiIndex::prepare_for_source(wiki_root, repo_root, source)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let repo = Self { dir };
            repo.git(&["init"]);
            repo.git(&["checkout", "-b", "main"]);
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
            fs::write(full, content).expect("write file");
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

}
