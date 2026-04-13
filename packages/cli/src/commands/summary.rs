use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::index::{ResolvedPage, SearchResult, WikiIndex};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SummaryOutput {
    pub title: String,
    pub file: String,
    pub summary: String,
    #[serde(skip_serializing)]
    pub alias: Option<String>,
}

fn summary_output(page: ResolvedPage) -> SummaryOutput {
    SummaryOutput {
        title: page.title,
        file: page.file,
        summary: page.summary,
        alias: page.alias,
    }
}

fn repo_relative(abs_path: &str, repo_root: &Path) -> String {
    Path::new(abs_path)
        .strip_prefix(repo_root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| abs_path.to_owned())
}

pub fn format_text_summary(output: &SummaryOutput, repo_root: &Path) -> String {
    let heading = match &output.alias {
        Some(alias) => format!("# {} (alias of {})", output.title, alias),
        None => format!("# {}", output.title),
    };
    let rel = repo_relative(&output.file, repo_root);
    format!("{heading}\n## {rel}\n{}", output.summary)
}

pub fn format_search_result(result: &SearchResult, repo_root: &Path) -> String {
    let rel = repo_relative(&result.file, repo_root);
    let mut rendered = format!("# {}\n## {rel}\n{}", result.title, result.summary);
    if !result.snippets.is_empty() {
        rendered.push_str("\n\nMatched snippets:");
        for snippet in &result.snippets {
            rendered.push_str(&format!("\n- L{}: {}", snippet.line, snippet.text));
        }
    }
    rendered
}

pub fn render_not_found(title: &str, suggestions: &[SearchResult], repo_root: &Path) -> String {
    if suggestions.is_empty() {
        return if title.contains('/') || title.ends_with(".md") {
            format!("No wiki page found at path `{title}`.")
        } else {
            format!("No page found with title or alias `{title}`.")
        };
    }

    let mut rendered = String::from("Did you mean:\n");
    for suggestion in suggestions {
        rendered.push('\n');
        rendered.push('\n');
        rendered.push_str(&format_search_result(suggestion, repo_root));
    }
    rendered
}

pub fn run(title: &str, json: bool, repo_root: &Path) -> Result<i32> {
    let index = WikiIndex::prepare(repo_root)?;
    match index.resolve_page(title)? {
        Some(page) => {
            let output = summary_output(page);
            if json {
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                println!("{}", format_text_summary(&output, repo_root));
            }
            Ok(0)
        }
        None => {
            let suggestions = index.suggest(title)?;
            if json {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "error": format!("page '{}' not found", title),
                        "suggestions": suggestions,
                    })
                );
            } else {
                eprintln!("{}", render_not_found(title, &suggestions, repo_root));
            }
            Ok(1)
        }
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
    fn resolves_summary_by_alias_from_index() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: My Page\naliases:\n  - alt\nsummary: This is the summary.\n---\nBody text.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let output = summary_output(index.resolve_page("alt").expect("resolve").expect("page"));
        assert_eq!(output.title, "My Page");
        assert_eq!(output.alias.as_deref(), Some("alt"));
    }

    #[test]
    fn not_found_message_uses_suggestions_when_available() {
        let suggestions = vec![SearchResult {
            title: "Example".into(),
            file: "/tmp/wiki/example.md".into(),
            summary: "Summary".into(),
            alias: None,
            snippets: vec![],
        }];

        let rendered = render_not_found("Exampel", &suggestions, std::path::Path::new("/tmp"));
        assert!(rendered.contains("Did you mean:"));
        assert!(rendered.contains("# Example"));
    }
}
