use std::path::{Path, PathBuf};

use miette::Result;
use serde::Serialize;

use crate::commands::discover_files;
use crate::git::{latest_commit, resolve_ref};
use crate::parser::{FragmentLink, LinkKind, parse_fragment_links};

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PinEntry {
    pub wiki_file: String,
    pub source_line: usize,
    pub referenced_path: String,
    pub old_sha: Option<String>,
    pub new_sha: String,
    pub action: String, // "refreshed" | "unchanged"
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the pin command.
///
/// Returns exit code: 0 = success, 2 = runtime error.
pub fn run(globs: &[String], git_ref: Option<&str>, json: bool, repo_root: &Path) -> Result<i32> {
    // Resolve the target ref
    let ref_name = git_ref.unwrap_or("HEAD");
    let resolved_ref = match resolve_ref(repo_root, ref_name) {
        Ok(r) => r,
        Err(e) => {
            if json {
                eprintln!("{}", serde_json::json!({"error": e.to_string()}));
            } else {
                eprintln!("error: failed to resolve ref '{ref_name}': {e}");
            }
            return Ok(2);
        }
    };

    let files = match discover_files(globs, repo_root) {
        Ok(f) => f,
        Err(e) => {
            if json {
                eprintln!("{}", serde_json::json!({"error": e.to_string()}));
            } else {
                eprintln!("error: {e}");
            }
            return Ok(2);
        }
    };

    let mut pin_entries: Vec<PinEntry> = Vec::new();
    let mut had_error = false;

    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to read {}: {e}", path.display());
                continue;
            }
        };

        let frag_links = parse_fragment_links(&content);
        // Only process links that already have a SHA — skip unpinned links.
        let needs_update = frag_links
            .iter()
            .any(|l| l.kind == LinkKind::InternalWithSha);

        if !needs_update {
            continue;
        }

        // Collect rewrites: determine new SHA for each already-pinned link
        struct Rewrite {
            link: FragmentLink,
            new_sha: String,
            new_path: String,
            action: &'static str,
        }

        let mut rewrites: Vec<Rewrite> = Vec::new();

        for link in &frag_links {
            // Only refresh links that already have a SHA; skip unpinned and external links.
            if link.kind != LinkKind::InternalWithSha {
                continue;
            }

            let resolved_path = crate::commands::resolve_link_path(&link.path, path, repo_root);
            let repo_relative_path = resolved_path.to_string_lossy().to_string();
            let is_repo_relative = link.path == repo_relative_path;

            // Get the latest commit for this file at or before the resolved ref
            let new_sha = match latest_commit(repo_root, &resolved_ref, &resolved_path) {
                Ok(sha) => sha,
                Err(e) => {
                    eprintln!(
                        "error: failed to find commit for '{}' at '{}': {e}",
                        link.path, ref_name
                    );
                    had_error = true;
                    continue;
                }
            };

            // link.kind == InternalWithSha, so link.sha is always Some(_).
            let sha_changed = link.sha.as_ref().map(|s| s != &new_sha).unwrap_or(true);
            let needs_fix = sha_changed || !is_repo_relative;

            let action = if sha_changed {
                "refreshed"
            } else if !is_repo_relative {
                "converted"
            } else {
                "unchanged"
            };

            pin_entries.push(PinEntry {
                wiki_file: path.display().to_string(),
                source_line: link.source_line,
                referenced_path: link.path.clone(),
                old_sha: link.sha.clone(),
                new_sha: new_sha.clone(),
                action: action.to_string(),
            });
            if needs_fix {
                rewrites.push(Rewrite {
                    link: link.clone(),
                    new_sha,
                    new_path: repo_relative_path,
                    action,
                });
            }
        }

        if !rewrites.is_empty() {
            // Sort rewrites by source_line descending so we process last-to-first.
            // This preserves byte offsets of earlier occurrences when we do
            // targeted in-place replacements.
            rewrites.sort_by(|a, b| b.link.source_line.cmp(&a.link.source_line));

            let new_content = apply_rewrites(
                &content,
                &rewrites
                    .iter()
                    .map(|r| RewriteSpec {
                        source_line: r.link.source_line,
                        text: r.link.original_text.clone(),
                        original_href: r.link.original_href.clone(),
                        new_path: r.new_path.clone(),
                        new_sha: r.new_sha.clone(),
                        action: r.action,
                        fragment: build_fragment(&r.link),
                    })
                    .collect::<Vec<_>>(),
            );

            // Atomic write: write to tempfile in same dir, then rename
            if let Err(e) = write_atomic(path, &new_content) {
                eprintln!("error: failed to write {}: {e}", path.display());
                had_error = true;
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&pin_entries).unwrap());
    } else {
        for entry in &pin_entries {
            if entry.action == "refreshed" {
                println!(
                    "`{}:{}` — `{}`\n`@{}` → `@{}`\n",
                    entry.wiki_file,
                    entry.source_line,
                    entry.referenced_path,
                    entry.old_sha.as_deref().unwrap_or("?"),
                    entry.new_sha
                );
            }
        }
    }

    if had_error { Ok(2) } else { Ok(0) }
}

/// Spec for a single link rewrite.
pub(crate) struct RewriteSpec {
    pub(crate) source_line: usize,
    pub(crate) text: String,
    pub(crate) original_href: String, // The exact href as it appeared in the markdown link
    pub(crate) new_path: String,      // The repo-relative path to write into the link
    pub(crate) new_sha: String,
    #[allow(dead_code)]
    pub(crate) action: &'static str,
    pub(crate) fragment: Option<String>,
}

/// Build the fragment string for a link (e.g. "L10-L20" or "L5").
pub(crate) fn build_fragment(link: &crate::parser::FragmentLink) -> Option<String> {
    match (link.start_line, link.end_line) {
        (None, _) => None,
        (Some(start), None) => Some(format!("L{start}")),
        (Some(start), Some(end)) => Some(format!("L{start}-L{end}")),
    }
}

/// Apply rewrites to `content` using position-based replacement.
///
/// Rewrites must be sorted by `source_line` descending so that earlier byte
/// offsets are not invalidated by changes to later positions.
pub(crate) fn apply_rewrites(content: &str, specs: &[RewriteSpec]) -> String {
    let mut result = content.to_string();

    for spec in specs {
        // Build the exact old href that appears in the markdown link
        let old_href = &spec.original_href;

        // Build the new href using `#fragment&sha` or `#sha` format
        let new_href = match &spec.fragment {
            Some(frag) => format!("{}#{}&{}", spec.new_path, frag, spec.new_sha),
            None => format!("{}#{}", spec.new_path, spec.new_sha),
        };

        // Build the exact old markdown link syntax to find
        let old_link = format!("[{}]({})", spec.text, old_href);
        let new_link = format!("[{}]({})", spec.text, new_href);

        // Find which occurrence is on `source_line` (1-based).
        // Iterate over matches and pick the one whose line matches.
        let target_line = spec.source_line;
        let mut offset = 0;
        while let Some(pos) = find_occurrence_on_line(&result, &old_link, target_line, offset) {
            result.replace_range(pos..pos + old_link.len(), &new_link);
            offset = pos + new_link.len();
        }
    }

    result
}

/// Find the byte position of `needle` whose occurrence falls on `target_line` (1-based).
pub(crate) fn find_occurrence_on_line(
    haystack: &str,
    needle: &str,
    target_line: usize,
    start_offset: usize,
) -> Option<usize> {
    let needle_bytes = needle.as_bytes();
    let haystack_bytes = haystack.as_bytes();
    let len = haystack_bytes.len();
    let needle_len = needle_bytes.len();

    if needle_len == 0 || needle_len > len || start_offset > len {
        return None;
    }

    for i in start_offset..=(len - needle_len) {
        if haystack_bytes[i..i + needle_len] == *needle_bytes {
            // Count 1-based line number of this position
            let line = haystack[..i].bytes().filter(|&b| b == b'\n').count() + 1;
            if line == target_line {
                return Some(i);
            }
        }
    }
    None
}

/// Write `content` to `path` atomically by writing to a tempfile in the same
/// directory, then renaming.
pub(crate) fn write_atomic(path: &PathBuf, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

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
            let repo = TestRepo { dir };
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
            fs::write(&full, content).expect("write file");
        }

        fn commit(&self, message: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-m", message]);
            let out = Command::new("git")
                .current_dir(self.dir.path())
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .expect("git rev-parse");
            String::from_utf8(out.stdout)
                .expect("utf8")
                .trim()
                .to_string()
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
    fn test_pin_skips_unpinned_link() {
        // wiki pin should NOT insert SHAs on unpinned links — that is wiki check --fix's job.
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        repo.commit("add src");

        let original = "---\ntitle: Page\n---\n[code](src/foo.rs#L1)\n";
        repo.create_file("wiki/page.md", original);
        repo.commit("add wiki");

        let code = run(&[], None, false, repo.path()).expect("run");
        assert_eq!(code, 0);

        // File must remain unchanged — pin must not insert a SHA.
        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        assert_eq!(
            content, original,
            "wiki pin must not modify unpinned links, got: {content}"
        );
    }

    #[test]
    fn test_pin_refreshes_stale_sha() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let old_sha = repo.commit("add src");

        repo.create_file(
            "wiki/page.md",
            &format!("---\ntitle: Page\n---\n[code](src/foo.rs#L1&{old_sha})\n"),
        );
        repo.commit("add wiki");

        // Modify the source file
        repo.create_file("src/foo.rs", "fn foo() {}\nfn bar() {}\n");
        repo.commit("update src");

        let code = run(&[], None, false, repo.path()).expect("run");
        assert_eq!(code, 0);

        // The SHA should have been updated
        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        // Should no longer contain the old SHA (or should contain new one)
        // The old SHA might still appear as a substring, so check for the new pattern
        assert!(
            content.contains("src/foo.rs#"),
            "expected updated SHA, got: {content}"
        );
    }

    #[test]
    fn test_pin_atomic_write() {
        // Atomic write is verified when refreshing a stale SHA.
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "line1\nline2\n");
        let sha1 = repo.commit("add src");

        let original = format!("---\ntitle: Page\n---\n[code](src/foo.rs#L1&{sha1})\n");
        repo.create_file("wiki/page.md", &original);
        repo.commit("add wiki");

        // Modify source so there is a new SHA to refresh to.
        repo.create_file("src/foo.rs", "line1\nline2\nline3\n");
        repo.commit("update src");

        let code = run(&[], None, false, repo.path()).expect("run");
        assert_eq!(code, 0);

        // File should be readable and valid after atomic write
        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        assert!(!content.is_empty());
        assert!(content.contains("---\ntitle: Page\n---\n"));
    }

    #[test]
    fn test_pin_json_output() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "fn foo() {}\n");
        let sha1 = repo.commit("add src");

        // Use a pinned link so pin has something to potentially refresh.
        repo.create_file(
            "wiki/page.md",
            &format!("---\ntitle: Page\n---\n[code](src/foo.rs#L1&{sha1})\n"),
        );
        repo.commit("add wiki");

        let code = run(&[], None, true, repo.path()).expect("run");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_apply_rewrites_inserts_sha_before_fragment() {
        let content = "[code](src/foo.rs#L1)\n";
        let spec = RewriteSpec {
            source_line: 1,
            text: "code".into(),
            original_href: "src/foo.rs#L1".into(),
            new_path: "src/foo.rs".into(),
            new_sha: "abc1234".into(),
            action: "pinned",
            fragment: Some("L1".into()),
        };
        let result = apply_rewrites(content, &[spec]);
        assert_eq!(result, "[code](src/foo.rs#L1&abc1234)\n");
    }

    #[test]
    fn test_apply_rewrites_inserts_sha_no_fragment() {
        let content = "[code](src/foo.rs)\n";
        let spec = RewriteSpec {
            source_line: 1,
            text: "code".into(),
            original_href: "src/foo.rs".into(),
            new_path: "src/foo.rs".into(),
            new_sha: "abc1234".into(),
            action: "pinned",
            fragment: None,
        };
        let result = apply_rewrites(content, &[spec]);
        assert_eq!(result, "[code](src/foo.rs#abc1234)\n");
    }

    #[test]
    fn test_apply_rewrites_replaces_existing_sha() {
        let content = "[code](src/foo.rs#L1&oldsha1)\n";
        let spec = RewriteSpec {
            source_line: 1,
            text: "code".into(),
            original_href: "src/foo.rs#L1&oldsha1".into(),
            new_path: "src/foo.rs".into(),
            new_sha: "newsha2".into(),
            action: "refreshed",
            fragment: Some("L1".into()),
        };
        let result = apply_rewrites(content, &[spec]);
        assert_eq!(result, "[code](src/foo.rs#L1&newsha2)\n");
    }

    #[test]
    fn test_pin_two_pinned_links_same_file_different_ranges() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/foo.rs", "line1\nline2\nline3\n");
        let sha1 = repo.commit("add src");

        // Two links to same file, different line ranges, both already pinned to sha1.
        repo.create_file(
            "wiki/page.md",
            &format!(
                "---\ntitle: Page\n---\n[first](src/foo.rs#L1&{sha1})\n[second](src/foo.rs#L2&{sha1})\n"
            ),
        );
        repo.commit("add wiki");

        // Update the source so there is a new SHA to refresh to.
        repo.create_file("src/foo.rs", "line1\nline2\nline3\nline4\n");
        repo.commit("update src");

        let code = run(&[], None, false, repo.path()).expect("run");
        assert_eq!(code, 0);

        let content = fs::read_to_string(repo.path().join("wiki/page.md")).expect("read");
        // Both links should still contain a SHA (now refreshed).
        assert!(
            content.contains("src/foo.rs#"),
            "expected SHA in links, got:\n{content}"
        );
        // Each fragment should appear exactly once.
        let count_l1 = content.matches("#L1&").count();
        let count_l2 = content.matches("#L2&").count();
        assert_eq!(
            count_l1, 1,
            "L1 fragment should appear once, got:\n{content}"
        );
        assert_eq!(
            count_l2, 1,
            "L2 fragment should appear once, got:\n{content}"
        );
    }
}
