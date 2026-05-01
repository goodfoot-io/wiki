use std::collections::{HashMap, HashSet};
use std::path::Path;

use miette::Result;
use serde::Serialize;

use crate::index::{DocSource, WikiIndex};
use crate::parser::parse_wikilinks;
use crate::wiki_config::WikiConfig;

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

/// Run `refs` across multiple namespaces sequentially. The source page is
/// looked up in each namespace independently; namespaces where the page is
/// missing are skipped. Output is labeled with the namespace.
pub fn run_multi(
    title: &str,
    json: bool,
    targets: &[(String, &Path)],
    repo_root: &Path,
    wiki_config: Option<&WikiConfig>,
    source: DocSource,
) -> Result<i32> {
    let mut any_resolved_source = false;
    if json {
        let mut out: Vec<serde_json::Value> = Vec::new();
        for (label, wiki_root) in targets {
            let index = WikiIndex::prepare_for_source(wiki_root, repo_root, source)?;
            let Some(page) = index.resolve_page(title)? else {
                continue;
            };
            any_resolved_source = true;
            let entries = collect_entries(&index, &page.content, wiki_config, repo_root)?;
            for e in &entries {
                let mut v = serde_json::to_value(e).unwrap();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("namespace".into(), serde_json::json!(label));
                }
                out.push(v);
            }
        }
        if !any_resolved_source {
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": format!("page '{}' not found in any namespace", title),
                })
            );
            return Ok(1);
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(0);
    }

    let mut first = true;
    for (label, wiki_root) in targets {
        let index = WikiIndex::prepare_for_source(wiki_root, repo_root, source)?;
        let Some(page) = index.resolve_page(title)? else {
            continue;
        };
        any_resolved_source = true;
        let entries = collect_entries(&index, &page.content, wiki_config, repo_root)?;
        if entries.is_empty() {
            continue;
        }
        if !first {
            println!("\n---\n");
        }
        first = false;
        println!("# [{label}]\n{}", format_text_entries(&entries));
    }
    if !any_resolved_source {
        eprintln!("No page found with title or alias `{title}` in any namespace.");
        return Ok(1);
    }
    Ok(0)
}

fn collect_entries(
    index: &WikiIndex,
    content: &str,
    wiki_config: Option<&WikiConfig>,
    repo_root: &Path,
) -> Result<Vec<RefEntry>> {
    let wikilinks = parse_wikilinks(content);
    let mut seen = HashSet::new();
    let mut targets: Vec<(Option<String>, String)> = Vec::new();
    for wl in wikilinks {
        let key = (
            wl.namespace.as_ref().map(|n| n.to_lowercase()),
            wl.title.to_lowercase(),
        );
        if seen.insert(key) {
            targets.push((wl.namespace, wl.title));
        }
    }

    let current_namespace = index.namespace();
    let mut peer_indices: HashMap<String, Option<WikiIndex>> = HashMap::new();
    let mut entries = Vec::with_capacity(targets.len());
    for (ns, title) in targets {
        let display = match &ns {
            Some(n) => format!("{n}:{title}"),
            None => title.clone(),
        };

        // Same-namespace link, or explicit prefix matching the current wiki.
        let resolve_locally = match &ns {
            None => true,
            Some(n) => current_namespace == Some(n.as_str()),
        };

        if resolve_locally {
            match index.resolve_page_full(&title)? {
                Some(full) => entries.push(RefEntry::Resolved {
                    wikilink: display,
                    title: full.title,
                    file: full.file,
                    summary: full.summary,
                    aliases: full.aliases,
                    tags: full.tags,
                }),
                None => entries.push(RefEntry::Unresolved {
                    wikilink: display,
                    error: "not found".to_string(),
                }),
            }
            continue;
        }

        let ns_name = ns.as_deref().expect("ns is Some when resolving cross-namespace");
        let peer_info = wiki_config.and_then(|cfg| cfg.wikis.get(ns_name));
        let Some(peer_info) = peer_info else {
            entries.push(RefEntry::Unresolved {
                wikilink: display,
                error: format!(
                    "namespace `{ns_name}` is not declared by any wiki.toml in this repo"
                ),
            });
            continue;
        };

        let peer_root = peer_info.root.clone();
        let peer_index_slot = peer_indices
            .entry(ns_name.to_string())
            .or_insert_with(|| WikiIndex::prepare(&peer_root, repo_root).ok());
        let Some(peer_index) = peer_index_slot.as_ref() else {
            entries.push(RefEntry::Unresolved {
                wikilink: display,
                error: format!("could not load namespace `{ns_name}`"),
            });
            continue;
        };

        match peer_index.resolve_page_full(&title)? {
            Some(full) => entries.push(RefEntry::Resolved {
                wikilink: display,
                title: full.title,
                file: full.file,
                summary: full.summary,
                aliases: full.aliases,
                tags: full.tags,
            }),
            None => entries.push(RefEntry::Unresolved {
                wikilink: display,
                error: format!("not found in namespace `{ns_name}`"),
            }),
        }
    }
    Ok(entries)
}

pub fn run(
    title: &str,
    json: bool,
    wiki_root: &Path,
    repo_root: &Path,
    wiki_config: Option<&WikiConfig>,
    source: DocSource,
) -> Result<i32> {
    let index = WikiIndex::prepare_for_source(wiki_root, repo_root, source)?;
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

    let entries = collect_entries(&index, &page.content, wiki_config, repo_root)?;

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
    use std::path::PathBuf;
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

    fn write_namespaced_wiki(repo_root: &Path, dir: &str, ns: &str) -> PathBuf {
        let wiki_root = repo_root.join(dir);
        fs::create_dir_all(&wiki_root).expect("create wiki dir");
        fs::write(
            wiki_root.join("wiki.toml"),
            format!("namespace = \"{ns}\"\n"),
        )
        .expect("write wiki.toml");
        wiki_root
    }

    fn build_entries(index: &WikiIndex, page_title: &str) -> Vec<RefEntry> {
        let page = index
            .resolve_page(page_title)
            .expect("resolve")
            .expect("page");
        // Mirror the production pipeline using a no-op config, so single-namespace
        // tests that don't need cross-namespace resolution behave as before.
        collect_entries(index, &page.content, None, index.repo_root()).expect("collect")
    }

    #[test]
    fn resolves_all_wikilinks_with_aliases_and_tags() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
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

        let index = WikiIndex::prepare(&wiki_root, repo.path()).expect("prepare");
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
            RefEntry::Resolved {
                wikilink,
                title,
                aliases,
                tags,
                ..
            } => {
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
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Source page.\n---\n[[Known]] [[Missing Page]]\n",
        );
        repo.create_file(
            "wiki/known.md",
            "---\ntitle: Known\nsummary: Exists.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(&wiki_root, repo.path()).expect("prepare");
        let entries = build_entries(&index, "Source");
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], RefEntry::Resolved { title, .. } if title == "Known"));
        assert!(
            matches!(&entries[1], RefEntry::Unresolved { wikilink, error } if wikilink == "Missing Page" && error == "not found")
        );
    }

    #[test]
    fn deduplicates_by_wikilink_target() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Source page.\n---\n[[Target]] and [[Target]] and [[target]]\n",
        );
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Target\nsummary: Target page.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(&wiki_root, repo.path()).expect("prepare");
        let entries = build_entries(&index, "Source");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn source_page_not_found_returns_exit_1() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/other.md",
            "---\ntitle: Other\nsummary: Other page.\n---\nBody.\n",
        );

        let code = run("Nonexistent", true, &wiki_root, repo.path(), None, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 1);
    }

    #[test]
    fn succeeds_with_exit_0_when_some_unresolved() {
        let repo = TestRepo::new();
        let wiki_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Source page.\n---\n[[Missing]]\n",
        );

        let code = run("Source", true, &wiki_root, repo.path(), None, crate::index::DocSource::WorkingTree).expect("run");
        assert_eq!(code, 0);
    }

    // ── Cross-namespace wikilink resolution (regression for main-9) ───────────

    #[test]
    fn cross_namespace_wikilink_resolves_against_peer_index() {
        let repo = TestRepo::new();
        let default_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        let _scratch_root = write_namespaced_wiki(repo.path(), "scratch", "scratch");
        repo.create_file(
            "wiki/authentication.md",
            "---\ntitle: Authentication\nsummary: Auth.\n---\nWe use OAuth2. See [[Sessions]] and [[scratch:Operator Notes]].\n",
        );
        repo.create_file(
            "wiki/sessions.md",
            "---\ntitle: Sessions\nsummary: Session lifecycle.\n---\nBody.\n",
        );
        repo.create_file(
            "scratch/operator-notes.md",
            "---\ntitle: Operator Notes\nsummary: Day-to-day operator notes.\n---\nBody.\n",
        );

        let cfg = WikiConfig::load(repo.path(), repo.path()).expect("config");
        let index = WikiIndex::prepare(&default_root, repo.path()).expect("prepare");
        let page = index
            .resolve_page("Authentication")
            .expect("resolve")
            .expect("page");
        let entries =
            collect_entries(&index, &page.content, Some(&cfg), repo.path()).expect("entries");

        assert_eq!(entries.len(), 2);
        let cross = entries
            .iter()
            .find(|e| matches!(e, RefEntry::Resolved { wikilink, .. } | RefEntry::Unresolved { wikilink, .. } if wikilink.starts_with("scratch:")))
            .expect("cross-namespace entry present with prefix preserved");
        match cross {
            RefEntry::Resolved {
                wikilink,
                title,
                summary,
                ..
            } => {
                assert_eq!(wikilink, "scratch:Operator Notes");
                assert_eq!(title, "Operator Notes");
                assert!(
                    summary.contains("operator"),
                    "summary should come from the peer page, got: {summary}"
                );
            }
            _ => panic!("expected scratch:Operator Notes to resolve against the peer index"),
        }
    }

    #[test]
    fn cross_namespace_unresolved_preserves_prefix_and_names_namespace() {
        let repo = TestRepo::new();
        let default_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        let _scratch_root = write_namespaced_wiki(repo.path(), "scratch", "scratch");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Src.\n---\nSee [[scratch:Missing]].\n",
        );

        let cfg = WikiConfig::load(repo.path(), repo.path()).expect("config");
        let index = WikiIndex::prepare(&default_root, repo.path()).expect("prepare");
        let page = index.resolve_page("Source").expect("resolve").expect("page");
        let entries =
            collect_entries(&index, &page.content, Some(&cfg), repo.path()).expect("entries");

        assert_eq!(entries.len(), 1);
        match &entries[0] {
            RefEntry::Unresolved { wikilink, error } => {
                assert_eq!(wikilink, "scratch:Missing");
                assert!(
                    error.contains("scratch"),
                    "error must name the scratch namespace, got: {error}"
                );
            }
            _ => panic!("expected unresolved entry"),
        }
    }

    #[test]
    fn cross_namespace_unknown_namespace_named_in_error() {
        let repo = TestRepo::new();
        let default_root = crate::test_support::write_wiki_toml(repo.path(), "wiki");
        repo.create_file(
            "wiki/source.md",
            "---\ntitle: Source\nsummary: Src.\n---\nSee [[unknown:X]].\n",
        );

        let cfg = WikiConfig::load(repo.path(), repo.path()).expect("config");
        let index = WikiIndex::prepare(&default_root, repo.path()).expect("prepare");
        let page = index.resolve_page("Source").expect("resolve").expect("page");
        let entries =
            collect_entries(&index, &page.content, Some(&cfg), repo.path()).expect("entries");

        assert_eq!(entries.len(), 1);
        match &entries[0] {
            RefEntry::Unresolved { wikilink, error } => {
                assert_eq!(wikilink, "unknown:X");
                assert!(
                    error.contains("unknown") && error.contains("not declared"),
                    "error must name the unknown namespace and note it's undeclared, got: {error}"
                );
            }
            _ => panic!("expected unresolved entry for unknown namespace"),
        }
    }
}
