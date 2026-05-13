use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::index::{DocSource, WikiIndex};

/// Incoming fragment-link backlink to a wiki page.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RefEntry {
    pub source_file: String,
    pub source_title: String,
    pub line: u32,
    pub text: String,
}

fn format_text_entries(entries: &[RefEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&format!(
            "## {} ({}:L{})\n{}\n\n",
            entry.source_title, entry.source_file, entry.line, entry.text
        ));
    }
    out.trim_end().to_string()
}

pub fn run(
    title: &str,
    json: bool,
    wiki_root: &Path,
    repo_root: &Path,
    source: DocSource,
) -> Result<i32> {
    let index = WikiIndex::prepare_for_source(wiki_root, repo_root, source)?;
    let Some(_page) = index.resolve_page(title)? else {
        if json {
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": format!("page '{}' not found", title),
                })
            );
        } else {
            eprintln!("No page found with title or alias `{title}`.");
        }
        return Ok(1);
    };

    // Incoming fragment-links: every page that references this one by path or title.
    let backlinks = index.links(title)?;
    let mut entries: Vec<RefEntry> = Vec::new();
    for result in backlinks {
        for snippet in result.snippets {
            entries.push(RefEntry {
                source_file: result.file.clone(),
                source_title: result.title.clone(),
                line: snippet.line as u32,
                text: snippet.text,
            });
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        println!("{}", format_text_entries(&entries));
    }
    Ok(0)
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

    #[test]
    fn source_page_not_found_returns_exit_1() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/other.md",
            "---\ntitle: Other\nsummary: Other page.\n---\nBody.\n",
        );

        let code = run(
            "Nonexistent",
            true,
            &wiki_root,
            repo.path(),
            crate::index::DocSource::WorkingTree,
        )
        .expect("run");
        assert_eq!(code, 1);
    }
}
