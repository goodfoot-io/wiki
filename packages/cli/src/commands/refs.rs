use std::collections::HashSet;
use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::index::WikiIndex;
use crate::parser::parse_wikilinks;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RefEntry {
    Resolved {
        wikilink: String,
        title: String,
        file: String,
        summary: String,
        aliases: Vec<String>,
        tags: Vec<String>,
    },
    Unresolved {
        wikilink: String,
        error: String,
    },
}

fn format_text_entries(entries: &[RefEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        match entry {
            RefEntry::Resolved {
                wikilink,
                title,
                file,
                summary,
                aliases,
                tags,
            } => {
                out.push_str(&format!("## [[{wikilink}]] -> {title}\n"));
                out.push_str(&format!("{file}\n"));
                if !aliases.is_empty() {
                    out.push_str(&format!("aliases: {}\n", aliases.join(", ")));
                }
                if !tags.is_empty() {
                    out.push_str(&format!("tags: {}\n", tags.join(", ")));
                }
                out.push_str(summary);
                out.push_str("\n\n");
            }
            RefEntry::Unresolved { wikilink, error } => {
                out.push_str(&format!("## [[{wikilink}]] -> {error}\n\n"));
            }
        }
    }
    out.trim_end().to_string()
}

pub fn run(title: &str, json: bool, repo_root: &Path) -> Result<i32> {
    let index = WikiIndex::prepare(repo_root)?;
    let Some(page) = index.resolve_page(title)? else {
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

    let wikilinks = parse_wikilinks(&page.content);
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for wikilink in wikilinks {
        let key = wikilink.title.to_lowercase();
        if seen.insert(key) {
            targets.push(wikilink.title);
        }
    }

    let mut entries = Vec::with_capacity(targets.len());
    for target in targets {
        match index.resolve_page_full(&target)? {
            Some(full) => entries.push(RefEntry::Resolved {
                wikilink: target,
                title: full.title,
                file: full.file,
                summary: full.summary,
                aliases: full.aliases,
                tags: full.tags,
            }),
            None => entries.push(RefEntry::Unresolved {
                wikilink: target,
                error: "not found".to_string(),
            }),
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

    fn build_entries(index: &WikiIndex, page_title: &str) -> Vec<RefEntry> {
        let page = index.resolve_page(page_title).expect("resolve").expect("page");
        let wikilinks = parse_wikilinks(&page.content);
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for wikilink in wikilinks {
            let key = wikilink.title.to_lowercase();
            if seen.insert(key) {
                targets.push(wikilink.title);
            }
        }

        let mut entries = Vec::new();
        for target in targets {
            match index.resolve_page_full(&target).expect("resolve_full") {
                Some(full) => entries.push(RefEntry::Resolved {
                    wikilink: target,
                    title: full.title,
                    file: full.file,
                    summary: full.summary,
                    aliases: full.aliases,
                    tags: full.tags,
                }),
                None => entries.push(RefEntry::Unresolved {
                    wikilink: target,
                    error: "not found".to_string(),
                }),
            }
        }
        entries
    }

    #[test]
    fn resolves_all_wikilinks_with_aliases_and_tags() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Source page.\n---\nSee [[Auth]] and [[Other Page]].\n",
        );
        repo.create_file(
            "wiki/auth.md",
            "---\ntitle: Authorization\naliases:\n  - Auth\ntags:\n  - security\n  - backend\nsummary: Auth overview.\n---\nBody.\n",
        );
        repo.create_file(
            "wiki/other.md",
            "---\ntitle: Other Page\nsummary: Other.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let entries = build_entries(&index, "Source");

        assert_eq!(entries.len(), 2);
        match &entries[0] {
            RefEntry::Resolved {
                wikilink,
                title,
                aliases,
                tags,
                ..
            } => {
                assert_eq!(wikilink, "Auth");
                assert_eq!(title, "Authorization");
                assert_eq!(aliases, &vec!["Auth".to_string()]);
                assert_eq!(tags, &vec!["backend".to_string(), "security".to_string()]);
            }
            _ => panic!("expected resolved"),
        }
        match &entries[1] {
            RefEntry::Resolved { wikilink, title, aliases, tags, .. } => {
                assert_eq!(wikilink, "Other Page");
                assert_eq!(title, "Other Page");
                assert!(aliases.is_empty());
                assert!(tags.is_empty());
            }
            _ => panic!("expected resolved"),
        }
    }

    #[test]
    fn mixed_resolved_and_unresolved_partial_results() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Source page.\n---\n[[Known]] [[Missing Page]]\n",
        );
        repo.create_file(
            "wiki/known.md",
            "---\ntitle: Known\nsummary: Exists.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let entries = build_entries(&index, "Source");
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], RefEntry::Resolved { title, .. } if title == "Known"));
        assert!(matches!(&entries[1], RefEntry::Unresolved { wikilink, error } if wikilink == "Missing Page" && error == "not found"));
    }

    #[test]
    fn deduplicates_by_wikilink_target() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Source page.\n---\n[[Target]] and [[Target]] and [[target]]\n",
        );
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Target\nsummary: Target page.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let entries = build_entries(&index, "Source");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn source_page_not_found_returns_exit_1() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/other.md",
            "---\ntitle: Other\nsummary: Other page.\n---\nBody.\n",
        );

        let code = run("Nonexistent", true, repo.path()).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn succeeds_with_exit_0_when_some_unresolved() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Source page.\n---\n[[Missing]]\n",
        );

        let code = run("Source", true, repo.path()).expect("run");
        assert_eq!(code, 0);
    }
}
