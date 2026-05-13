use std::path::Path;

use miette::Result;

use crate::commands::looks_like_path;
use crate::index::{DocSource, WikiIndex};

use super::summary::{format_search_result, render_not_found};

pub fn run(target: &str, json: bool, wiki_root: &Path, repo_root: &Path, source: DocSource) -> Result<i32> {
    let index = WikiIndex::prepare_for_source(wiki_root, repo_root, source)?;
    let matches = index.links(target)?;

    if matches.is_empty() {
        if !looks_like_path(target) && index.resolve_page(target)?.is_none() {
            let suggestions = index.suggest(target)?;
            if json {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "error": format!("page '{}' not found", target),
                        "suggestions": suggestions,
                    })
                );
            } else {
                eprintln!("{}", render_not_found(target, &suggestions, repo_root));
            }
            return Ok(0);
        }

        if json {
            println!("[]");
        }
        return Ok(0);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&matches).unwrap());
    } else {
        for (i, result) in matches.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("{}", format_search_result(result, repo_root));
        }
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
    fn returns_pages_linking_to_a_wiki_page() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Target Page\nsummary: Target summary.\n---\nBody.\n",
        );
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source Page\nsummary: Source summary.\n---\nSee [[Target Page]] for context.\n",
        );

        let code = run("Target Page", false, &wiki_root, repo.path(), crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);

        let results = WikiIndex::prepare(&wiki_root, repo.path())
            .expect("prepare")
            .links("Target Page")
            .expect("links");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Source Page");
        assert_eq!(results[0].snippets[0].line, 5);
        assert!(results[0].snippets[0].text.contains("[[Target Page]]"));
    }

    #[test]
    fn returns_pages_referencing_a_file() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("packages/foo/bar.ts", "export const x = 1;");
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: Foo Bar\nsummary: Describes the foo bar module.\n---\nSee [bar](/packages/foo/bar.ts) for details.\n",
        );

        let code = run("packages/foo/bar.ts", false, &wiki_root, repo.path(), crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);

        let results = WikiIndex::prepare(&wiki_root, repo.path())
            .expect("prepare")
            .links("packages/foo/bar.ts")
            .expect("links");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Foo Bar");
        assert_eq!(results[0].snippets[0].line, 5);
        assert!(results[0].snippets[0].text.contains("bar"));
    }

    #[test]
    fn path_input_can_return_page_links_and_file_refs() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Target Page\nsummary: Target summary.\n---\nBody.\n",
        );
        repo.create_file(
            "wiki/backlink.md",
            "---\ntitle: Backlink Page\nsummary: Links to the target page.\n---\nSee [[Target Page]].\n",
        );
        repo.create_file(
            "wiki/reference.md",
            "---\ntitle: Reference Page\nsummary: References the target file.\n---\nRead [the file](/wiki/target.md) directly.\n",
        );

        let results = WikiIndex::prepare(&wiki_root, repo.path())
            .expect("prepare")
            .links("wiki/target.md")
            .expect("links");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Backlink Page");
        assert_eq!(results[1].title, "Reference Page");
    }

    #[test]
    fn returns_exit_0_when_no_references_found() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: Page\nsummary: A page.\n---\nNo references.\n",
        );

        let code = run("packages/nonexistent/file.ts", false, &wiki_root, repo.path(), crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn strips_leading_dot_slash_from_path_input() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file("packages/foo/bar.ts", "export const x = 1;");
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: Foo Bar\nsummary: Summary.\n---\nSee [bar](packages/foo/bar.ts).\n",
        );

        let code = run("./packages/foo/bar.ts", false, &wiki_root, repo.path(), crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);
    }
}
