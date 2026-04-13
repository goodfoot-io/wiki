use std::path::Path;

use miette::Result;

use crate::index::WikiIndex;
use crate::render::{RenderMode, render_html, wrap_page};

use super::summary::render_not_found;

pub fn render_page(
    title: &str,
    fragment: bool,
    file_base_url: Option<&str>,
    repo_root: &Path,
) -> Result<Option<String>> {
    let index = WikiIndex::prepare(repo_root)?;
    let Some(page) = index.resolve_page(title)? else {
        return Ok(None);
    };

    let mode = if fragment {
        RenderMode::Fragment {
            file_base_url: file_base_url.map(ToOwned::to_owned),
        }
    } else {
        RenderMode::FullPage
    };
    let body = render_html(&page.content, mode, &index);
    Ok(Some(if fragment {
        body
    } else {
        wrap_page(&page.title, &body, false)
    }))
}

pub fn run(
    title: &str,
    fragment: bool,
    file_base_url: Option<&str>,
    repo_root: &Path,
) -> Result<i32> {
    let index = WikiIndex::prepare(repo_root)?;
    if index.resolve_page(title)?.is_some() {
        if let Some(output) = render_page(title, fragment, file_base_url, repo_root)? {
            print!("{output}");
            Ok(0)
        } else {
            unreachable!("page existence was checked immediately before rendering");
        }
    } else {
        let suggestions = index.suggest(title)?;
        eprintln!("{}", render_not_found(title, &suggestions, repo_root));
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

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
                fs::create_dir_all(parent).expect("create dirs");
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
    fn render_page_wraps_full_html_output() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Summary.\n---\nHello [[Other]].\n",
        );
        repo.create_file(
            "wiki/other.md",
            "---\ntitle: Other\nsummary: Other summary.\n---\nBody.\n",
        );

        let output = render_page("Example", false, None, repo.path())
            .expect("render")
            .expect("page");

        assert!(output.contains("<!doctype html>"));
        assert!(output.contains("Hello"));
        assert!(output.contains("/Other"));
    }
}
