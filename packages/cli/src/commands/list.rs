use std::path::Path;

use miette::Result;
use serde::Serialize;
use serde_json::json;

use crate::index::{PageListEntry, WikiIndex};

#[derive(Debug, Serialize)]
pub struct PageEntry {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub summary: String,
    pub file: String,
}

/// Run `list` across multiple namespaces sequentially. `targets` is
/// `(namespace_label, wiki_root)` in dispatch order. Output is labeled by namespace.
pub fn run_multi(tag: Option<&str>, json: bool, targets: &[(String, &Path)], repo_root: &Path) -> Result<i32> {
    if json {
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (label, wiki_root) in targets {
            let index = WikiIndex::prepare(wiki_root, repo_root)?;
            let entries = index.list_pages(tag)?.into_iter().map(page_entry).collect::<Vec<_>>();
            for entry in entries {
                let mut v = serde_json::to_value(&entry).unwrap();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("namespace".into(), json!(label));
                }
                out.push(v);
            }
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(0);
    }

    for (label, wiki_root) in targets {
        let index = WikiIndex::prepare(wiki_root, repo_root)?;
        let entries = index.list_pages(tag)?.into_iter().map(page_entry).collect::<Vec<_>>();
        for entry in &entries {
            println!("[{label}] **{}** — `{}`", entry.title, entry.file);
            let mut meta = Vec::new();
            if !entry.aliases.is_empty() {
                meta.push(format!(
                    "aliases: {}",
                    entry.aliases.iter().map(|a| format!("`{a}`")).collect::<Vec<_>>().join(", ")
                ));
            }
            if !entry.tags.is_empty() {
                meta.push(format!(
                    "tags: {}",
                    entry.tags.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ")
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
    fn run_multi_text_output_exits_zero_for_two_namespaces() {
        let repo = TestRepo::new();
        let wiki_a = crate::test_support::write_wiki_toml(repo.path(), "wiki-a");
        repo.create_file(
            "wiki-a/alpha.md",
            "---\ntitle: Alpha\ntags: [\"core\"]\nsummary: Alpha page.\n---\nBody.\n",
        );
        let wiki_b = crate::test_support::write_wiki_toml(repo.path(), "wiki-b");
        repo.create_file(
            "wiki-b/beta.md",
            "---\ntitle: Beta\ntags: [\"extra\"]\nsummary: Beta page.\n---\nBody.\n",
        );
        let targets: Vec<(String, &Path)> = vec![
            ("ns-a".to_string(), wiki_a.as_path()),
            ("ns-b".to_string(), wiki_b.as_path()),
        ];
        let code = run_multi(None, false, &targets, repo.path()).expect("run_multi");
        assert_eq!(code, 0);
    }

    #[test]
    fn run_multi_json_exits_zero_for_two_namespaces() {
        let repo = TestRepo::new();
        let wiki_a = crate::test_support::write_wiki_toml(repo.path(), "wiki-a");
        repo.create_file(
            "wiki-a/alpha.md",
            "---\ntitle: Alpha\nsummary: Alpha page.\n---\nBody.\n",
        );
        let wiki_b = crate::test_support::write_wiki_toml(repo.path(), "wiki-b");
        repo.create_file(
            "wiki-b/beta.md",
            "---\ntitle: Beta\nsummary: Beta page.\n---\nBody.\n",
        );
        let targets: Vec<(String, &Path)> = vec![
            ("ns-a".to_string(), wiki_a.as_path()),
            ("ns-b".to_string(), wiki_b.as_path()),
        ];
        let code = run_multi(None, true, &targets, repo.path()).expect("run_multi json");
        assert_eq!(code, 0);
    }

    #[test]
    fn run_multi_tag_filter_exits_zero() {
        let repo = TestRepo::new();
        let wiki_a = crate::test_support::write_wiki_toml(repo.path(), "wiki-a");
        repo.create_file(
            "wiki-a/alpha.md",
            "---\ntitle: Alpha\ntags: [\"core\"]\nsummary: Alpha page.\n---\nBody.\n",
        );
        let wiki_b = crate::test_support::write_wiki_toml(repo.path(), "wiki-b");
        repo.create_file(
            "wiki-b/beta.md",
            "---\ntitle: Beta\ntags: [\"extra\"]\nsummary: Beta page.\n---\nBody.\n",
        );
        let targets: Vec<(String, &Path)> = vec![
            ("ns-a".to_string(), wiki_a.as_path()),
            ("ns-b".to_string(), wiki_b.as_path()),
        ];
        // Filter by "core" tag — only alpha matches; should still exit 0
        let code = run_multi(Some("core"), false, &targets, repo.path()).expect("run_multi tag");
        assert_eq!(code, 0);
    }
}
