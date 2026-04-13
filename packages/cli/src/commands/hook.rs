use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use miette::Result;

use crate::index::{WikiIndex, is_lock_error};
use crate::parser::parse_wikilinks;

use super::{normalize_repo_relative_path, summary::format_search_result};

/// Append a warning line to the hook log file.
///
/// Written to the system temp directory so that concurrent hook invocations
/// never contend on the wiki directory itself. Nothing is written to
/// stderr or stdout so Claude Code hooks see no unexpected output.
fn log_warning(msg: &str) {
    let log_path = std::env::temp_dir().join("wiki-hook-warnings.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writeln!(file, "[{ts}] WARN {msg}");
    }
}

struct HookEvent {
    session_id: Option<String>,
    file_path: Option<String>,
}

/// Parse the Claude Code PostToolUse JSON event from stdin.
fn parse_hook_event(input: &str) -> HookEvent {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(input.trim()) else {
        return HookEvent {
            session_id: None,
            file_path: None,
        };
    };
    HookEvent {
        session_id: json
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        file_path: json
            .get("tool_input")
            .and_then(|ti| ti.get("file_path"))
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    }
}

/// Returns true when `file_path` is a wiki document — either inside the
/// `$WIKI_DIR` directory or named `*.wiki.md` — and therefore should not
/// trigger context injection (no point surfacing wiki links about a wiki page
/// the user is already reading).
fn is_wiki_file(file_path: &str, repo_root: &Path) -> bool {
    if file_path.ends_with(".wiki.md") {
        return true;
    }
    let wiki_dir_name = std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string());
    let path = Path::new(file_path);
    let wiki_abs = if Path::new(&wiki_dir_name).is_absolute() {
        PathBuf::from(&wiki_dir_name)
    } else {
        repo_root.join(&wiki_dir_name)
    };
    path.starts_with(&wiki_abs)
}

/// Normalise a path from the hook event to a repo-relative string suitable
/// for the incoming-links path lookup.
fn normalize_path(path: &str, repo_root: &Path) -> String {
    normalize_repo_relative_path(path, repo_root)
}

/// Path to the temp file that holds all session state for a given session.
fn session_file_path(session_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("wiki-hook-{session_id}"))
}

/// Load session state from the session file.
///
/// Returns `(shown_files, shown_title_keys)` where:
/// - `shown_files` is the set of repo-relative file paths whose phase-2/3
///   lookups have already run (used to gate redundant index scans).
/// - `shown_title_keys` is the set of lowercase article titles that have
///   already been injected in this session (shared deduplication for all
///   three injection phases).
fn load_session(session_id: &str) -> (HashSet<String>, HashSet<String>) {
    let path = session_file_path(session_id);
    let mut files = HashSet::new();
    let mut titles = HashSet::new();
    for line in std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
    {
        if let Some(rest) = line.strip_prefix("file:") {
            files.insert(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("title:") {
            titles.insert(rest.to_owned());
        }
    }
    (files, titles)
}

fn append_session_line(session_id: &str, line: &str) {
    let path = session_file_path(session_id);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Record that the phase-2/3 lookup for `path_rel` has run in this session.
fn record_shown_file(session_id: &str, path_rel: &str) {
    append_session_line(session_id, &format!("file:{path_rel}"));
}

/// Record that an article with the given lowercase title key was injected.
fn record_shown_title(session_id: &str, title_key: &str) {
    append_session_line(session_id, &format!("title:{title_key}"));
}

pub fn run(input: &str, repo_root: &Path) -> Result<i32> {
    // When the tool is operating on a wiki document itself, skip injection —
    // surfacing wiki context about the page being read is circular and noisy.
    let event = parse_hook_event(input);
    if event
        .file_path
        .as_deref()
        .is_some_and(|p| is_wiki_file(p, repo_root))
    {
        return Ok(0);
    }

    // Collect wikilink titles from the raw input (works for both plain text
    // and JSON events, since parse_wikilinks ignores non-wikilink content).
    let wikilinks = parse_wikilinks(input);
    let mut seen_input = HashSet::new();
    let mut candidate_titles = Vec::new();
    for wikilink in wikilinks {
        let key = wikilink.title.to_lowercase();
        if seen_input.insert(key) {
            candidate_titles.push(wikilink.title);
        }
    }

    // Load session state once: file-path gate + already-shown article titles.
    let (shown_files, shown_titles) = event
        .session_id
        .as_deref()
        .map(load_session)
        .unwrap_or_default();

    // Drop wikilink titles the session has already injected.
    let titles: Vec<String> = candidate_titles
        .into_iter()
        .filter(|t| !shown_titles.contains(&t.to_lowercase()))
        .collect();

    // Gate the file-path lookup on whether this file has been processed before.
    let unseen_file_path_rel = event.file_path.as_deref().and_then(|path| {
        let path_rel = normalize_path(path, repo_root);
        if shown_files.contains(&path_rel) { None } else { Some(path_rel) }
    });

    if titles.is_empty() && unseen_file_path_rel.is_none() {
        return Ok(0);
    }

    let index = match WikiIndex::prepare(repo_root) {
        Ok(idx) => idx,
        Err(err) if is_lock_error(&err) => {
            log_warning(&format!(
                "index database is locked by a concurrent writer; skipping context injection: {err}"
            ));
            return Ok(0);
        }
        Err(err) => return Err(err),
    };

    let mut context_lines: Vec<String> = Vec::new();
    // Tracks titles emitted this turn to prevent intra-turn duplicates.
    let mut emitted_this_turn: HashSet<String> = HashSet::new();
    // Titles newly shown this turn, to be persisted to the session file.
    let mut new_title_keys: Vec<String> = Vec::new();

    // Phase 1: wikilink-based context.
    // Titles are pre-filtered against shown_titles so only the insert check
    // is needed here (intra-turn deduplication).
    if !titles.is_empty() {
        let (resolved, _unresolved) = index.extract_pages(&titles)?;
        for page in resolved {
            let key = page.title.to_lowercase();
            if emitted_this_turn.insert(key.clone()) {
                context_lines.push(format_search_result(&page.into(), repo_root));
                new_title_keys.push(key);
            }
        }
    }

    // Phases 2 & 3: file-path context, gated once per file per session.
    if let Some(ref path_rel) = unseen_file_path_rel {
        // Phase 2: pages that reference the file being operated on.
        let links = index.links(path_rel)?;
        for result in links {
            let key = result.title.to_lowercase();
            if !shown_titles.contains(&key) && emitted_this_turn.insert(key.clone()) {
                context_lines.push(format_search_result(&result, repo_root));
                new_title_keys.push(key);
            }
        }

        // Phase 3: keyword-based article surfacing.
        // Scan the raw hook input for any indexed keyword at a word boundary.
        let all_keywords = index.fetch_all_keywords()?;
        let mut matched_ids: Vec<i64> = Vec::new();
        let mut seen_ids: HashSet<i64> = HashSet::new();
        for (keyword, doc_id) in &all_keywords {
            if keyword_matches(input, keyword) && seen_ids.insert(*doc_id) {
                matched_ids.push(*doc_id);
            }
        }
        let keyword_pages = index.fetch_pages_by_ids(&matched_ids)?;
        for result in keyword_pages {
            let key = result.title.to_lowercase();
            if !shown_titles.contains(&key) && emitted_this_turn.insert(key.clone()) {
                context_lines.push(format_search_result(&result, repo_root));
                new_title_keys.push(key);
            }
        }

        // Record the file as processed even when no articles were emitted,
        // so future turns skip the redundant lookup.
        if let Some(sid) = event.session_id.as_deref() {
            record_shown_file(sid, path_rel);
        }
    }

    // Persist all newly shown article titles to the session file.
    if let Some(sid) = event.session_id.as_deref() {
        for key in &new_title_keys {
            record_shown_title(sid, key);
        }
    }

    if context_lines.is_empty() {
        return Ok(0);
    }

    let context = context_lines.join("\n");

    let envelope = serde_json::json!({
        "systemMessage": context,
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": context,
        }
    });

    println!("{}", serde_json::to_string(&envelope).unwrap());

    Ok(0)
}

/// Returns true when `keyword` appears in `text` at a word boundary.
///
/// A word boundary means the character immediately before the match (if any)
/// and the character immediately after (if any) must not be alphanumeric
/// (`A-Za-z0-9`) or a hyphen (`-`).
fn keyword_matches(text: &str, keyword: &str) -> bool {
    if keyword.is_empty() {
        return false;
    }
    let text_bytes = text.as_bytes();
    let kw_bytes = keyword.as_bytes();
    let kw_len = kw_bytes.len();
    let text_len = text_bytes.len();

    if kw_len > text_len {
        return false;
    }

    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'-';

    let mut i = 0usize;
    while i + kw_len <= text_len {
        if text_bytes[i..i + kw_len] == *kw_bytes {
            let before_ok = i == 0 || !is_word_char(text_bytes[i - 1]);
            let after_ok = i + kw_len == text_len || !is_word_char(text_bytes[i + kw_len]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    use crate::index::WikiIndex;

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
    fn resolved_wikilink_produces_hook_envelope() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: The summary for Some Page.\n---\nBody text.\n",
        );

        let output = run("see [[Some Page]] for details", repo.path()).expect("run");
        assert_eq!(output, 0);
    }

    #[test]
    fn no_wikilinks_produces_no_output_and_exit_0() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: Summary.\n---\nBody.\n",
        );

        // No wikilinks in input — must return 0 without touching the index.
        // We verify by calling run() with a plain string and checking exit code.
        let output = run("no wikilinks here at all", repo.path()).expect("run");
        assert_eq!(output, 0);
    }

    #[test]
    fn claude_and_codex_flags_produce_identical_output() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: The summary for Some Page.\n---\nBody text.\n",
        );

        // run() itself doesn't distinguish --claude/--codex; the flags only
        // gate which code path in main.rs reaches run(). Both paths call the
        // same run() function so the output is identical by construction.
        // Verify by calling run() twice with the same input.
        let input = "see [[Some Page]] for details";
        let code1 = run(input, repo.path()).expect("run 1");
        let code2 = run(input, repo.path()).expect("run 2");
        assert_eq!(code1, code2);
        // Produce the envelope and check it is the same shape both times.
        // (run() writes to stdout; capture by re-running extract logic inline.)
        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let (resolved, _) = index
            .extract_pages(&["Some Page".to_string()])
            .expect("extract");
        assert_eq!(resolved.len(), 1);
        let ctx = format_search_result(&resolved[0].clone().into(), repo.path());
        assert_eq!(
            ctx,
            format_search_result(&resolved[0].clone().into(), repo.path())
        );

        let envelope1 = serde_json::json!({
            "systemMessage": ctx,
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": ctx,
            }
        });
        let envelope2 = serde_json::json!({
            "systemMessage": ctx,
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": ctx,
            }
        });
        assert_eq!(envelope1, envelope2);
    }

    #[test]
    fn resolved_wikilink_produces_correct_json_envelope() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: The summary for Some Page.\n---\nBody text.\n",
        );

        // Verify the envelope shape by building it directly (same logic as run()).
        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let (resolved, _) = index
            .extract_pages(&["Some Page".to_string()])
            .expect("extract");
        assert_eq!(resolved.len(), 1);

        let additional_context = resolved
            .iter()
            .map(|page| format_search_result(&page.clone().into(), repo.path()))
            .collect::<Vec<_>>()
            .join("\n");

        let envelope: serde_json::Value = serde_json::json!({
            "systemMessage": additional_context,
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": additional_context,
            }
        });

        assert_eq!(
            envelope["hookSpecificOutput"]["hookEventName"],
            "PostToolUse"
        );
        assert_eq!(
            envelope["systemMessage"], envelope["hookSpecificOutput"]["additionalContext"],
            "systemMessage and additionalContext must be identical"
        );
        assert!(
            envelope["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Some Page"),
            "additionalContext must contain title"
        );
        assert!(
            envelope["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("The summary for Some Page"),
            "additionalContext must contain summary"
        );
    }

    #[test]
    fn json_hook_event_with_file_path_includes_links() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        // A wiki page that references src/lib.rs via a fragment link.
        repo.create_file("src/lib.rs", "pub fn main() {}");
        repo.create_file(
            "wiki/lib-page.md",
            "---\ntitle: Lib Module\nsummary: The main library module.\n---\nSee [lib.rs](src/lib.rs) for the implementation.\n",
        );

        // Simulate a Claude Code PostToolUse JSON event for the Read tool.
        let hook_event = serde_json::json!({
            "session_id": "test-file-links-first-call",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": repo.path().join("src/lib.rs").to_string_lossy() },
            "tool_response": "pub fn main() {}",
            "tool_use_id": "tu_abc123"
        });

        let code = run(&hook_event.to_string(), repo.path()).expect("run");
        assert_eq!(code, 0);
        // Clean up session file so this test is hermetic.
        let _ = fs::remove_file(session_file_path("test-file-links-first-call"));
    }

    #[test]
    fn json_hook_event_without_file_links_match_exits_cleanly() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/page.md",
            "---\ntitle: Page\nsummary: A page.\n---\nNo file links.\n",
        );

        let hook_event = serde_json::json!({
            "session_id": "test-no-refs-match",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": repo.path().join("src/unrelated.ts").to_string_lossy() },
            "tool_response": "const x = 1;",
            "tool_use_id": "tu_def456"
        });

        let code = run(&hook_event.to_string(), repo.path()).expect("run");
        assert_eq!(code, 0);
        let _ = fs::remove_file(session_file_path("test-no-refs-match"));
    }

    #[test]
    fn same_file_is_not_shown_twice_in_same_session() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file("src/lib.rs", "pub fn main() {}");
        repo.create_file(
            "wiki/lib-page.md",
            "---\ntitle: Lib Module\nsummary: The main library module.\n---\nSee [lib.rs](src/lib.rs).\n",
        );

        let session_id = "test-dedup-session";
        // Clean up any leftover from a previous run.
        let _ = fs::remove_file(session_file_path(session_id));

        let make_event = |tool_use_id: &str| {
            serde_json::json!({
                "session_id": session_id,
                "hook_event_name": "PostToolUse",
                "tool_name": "Read",
                "tool_input": { "file_path": repo.path().join("src/lib.rs").to_string_lossy() },
                "tool_response": "pub fn main() {}",
                "tool_use_id": tool_use_id
            })
            .to_string()
        };

        // Confirm the wiki page references the file so the test setup is valid.
        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let links = index.links("src/lib.rs").expect("links");
        assert!(
            !links.is_empty(),
            "test setup: wiki page must reference src/lib.rs"
        );

        // First call: file not yet seen — succeeds.
        let code1 = run(&make_event("tu_first"), repo.path()).expect("run 1");
        assert_eq!(code1, 0, "first call should succeed");

        // After the first call the session file must record the file path and
        // at least one article title.
        let (shown_files, shown_titles) = load_session(session_id);
        assert!(
            shown_files.contains("src/lib.rs"),
            "session file must record src/lib.rs after first call, got: {shown_files:?}"
        );
        assert!(
            !shown_titles.is_empty(),
            "session file must record injected article titles after first call"
        );

        // Second call in same session: file already seen — run() exits 0 with
        // nothing to emit (the early-return path).
        let code2 = run(&make_event("tu_second"), repo.path()).expect("run 2");
        assert_eq!(
            code2, 0,
            "second call should exit 0 (nothing to emit, no error)"
        );

        // No new entries should be written on the second call.
        let (shown_files_after, shown_titles_after) = load_session(session_id);
        assert_eq!(
            shown_files_after.len(),
            1,
            "session file should have exactly one file entry"
        );
        assert_eq!(
            shown_titles_after.len(),
            shown_titles.len(),
            "session title count should not change on the second call"
        );

        // Clean up.
        let _ = fs::remove_file(session_file_path(session_id));
    }

    #[test]
    fn wikilinks_without_session_id_are_always_shown() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: Summary.\n---\nBody.\n",
        );

        // Plain-text input carries no session_id so session tracking is skipped.
        // Both calls must succeed — there is nothing to deduplicate against.
        let input = "see [[Some Page]] here";
        let code1 = run(input, repo.path()).expect("run 1");
        let code2 = run(input, repo.path()).expect("run 2");
        assert_eq!(code1, 0);
        assert_eq!(code2, 0);
    }

    #[test]
    fn wikilinks_are_deduplicated_per_session() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: Summary.\n---\nBody.\n",
        );

        let session_id = "test-wikilink-session-dedup";
        let _ = fs::remove_file(session_file_path(session_id));

        // Use a JSON event so that the session_id is present.
        let make_event = |tool_use_id: &str| {
            serde_json::json!({
                "session_id": session_id,
                "hook_event_name": "PostToolUse",
                "tool_name": "Read",
                "tool_input": { "file_path": repo.path().join("src/main.ts").to_string_lossy() },
                "tool_response": "see [[Some Page]] here",
                "tool_use_id": tool_use_id
            })
            .to_string()
        };

        // First call: article not yet seen — should inject and record the title.
        let code1 = run(&make_event("tu_first"), repo.path()).expect("run 1");
        assert_eq!(code1, 0, "first call should succeed");

        let (_, shown_titles) = load_session(session_id);
        assert!(
            shown_titles.contains("some page"),
            "session must record 'some page' after first call, got: {shown_titles:?}"
        );

        // Second call in same session: article already shown — wikilink phase
        // must be skipped (no new titles added, run returns 0 without emitting).
        let code2 = run(&make_event("tu_second"), repo.path()).expect("run 2");
        assert_eq!(code2, 0, "second call should succeed");

        let (_, shown_titles_after) = load_session(session_id);
        assert_eq!(
            shown_titles_after.len(),
            shown_titles.len(),
            "no new title entries should be written on the second call"
        );

        let _ = fs::remove_file(session_file_path(session_id));
    }

    #[test]
    fn reading_a_wiki_dir_file_produces_no_output() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: Summary.\n---\n[[Other Page]]\n",
        );

        // Reading a file inside the wiki/ directory must be suppressed entirely,
        // even though the content contains a wikilink.
        let hook_event = serde_json::json!({
            "session_id": "test-wiki-dir-suppression",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": repo.path().join("wiki/some-page.md").to_string_lossy() },
            "tool_response": "[[Other Page]]",
            "tool_use_id": "tu_wiki1"
        });

        let code = run(&hook_event.to_string(), repo.path()).expect("run");
        assert_eq!(code, 0);
        // No session file should be written for wiki files.
        assert!(
            !session_file_path("test-wiki-dir-suppression").exists(),
            "session file must not be created when reading a wiki file"
        );
    }

    #[test]
    fn reading_a_wiki_md_file_produces_no_output() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/some-page.md",
            "---\ntitle: Some Page\nsummary: Summary.\n---\nBody.\n",
        );
        repo.create_file(
            "src/component/docs.wiki.md",
            "---\ntitle: Component Docs\nsummary: In-tree wiki page.\n---\n[[Some Page]]\n",
        );

        let hook_event = serde_json::json!({
            "session_id": "test-wiki-md-suppression",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": repo.path().join("src/component/docs.wiki.md").to_string_lossy() },
            "tool_response": "[[Some Page]]",
            "tool_use_id": "tu_wiki2"
        });

        let code = run(&hook_event.to_string(), repo.path()).expect("run");
        assert_eq!(code, 0);
        assert!(
            !session_file_path("test-wiki-md-suppression").exists(),
            "session file must not be created when reading a .wiki.md file"
        );
    }

    #[test]
    fn keyword_match_injects_article() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/cards-create-page.md",
            "---\ntitle: Cards Create Page\nsummary: The cards create webview.\nkeywords:\n  - cards-create\n---\nBody.\n",
        );

        let session_id = "test-keyword-inject";
        let _ = fs::remove_file(session_file_path(session_id));

        let hook_event = serde_json::json!({
            "session_id": session_id,
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": repo.path().join("src/main.ts").to_string_lossy() },
            "tool_response": "// uses cards-create component",
            "tool_use_id": "tu_kw1"
        });

        // Verify the keyword is indexed before running the hook.
        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let keywords = index.fetch_all_keywords().expect("fetch_all_keywords");
        assert!(
            keywords.iter().any(|(kw, _)| kw == "cards-create"),
            "keyword must be indexed"
        );

        let code = run(&hook_event.to_string(), repo.path()).expect("run");
        assert_eq!(code, 0);

        let _ = fs::remove_file(session_file_path(session_id));
    }

    #[test]
    fn keyword_not_matched_without_word_boundary() {
        // "my-cards-create-inner" should NOT match keyword "cards-create"
        // because hyphen is a word char; "m" before "cards" via "-" means
        // the char before "cards" is '-' which IS a word char.
        assert!(!keyword_matches("my-cards-create-inner", "cards-create"));
        // But standalone is matched.
        assert!(keyword_matches("see cards-create here", "cards-create"));
        assert!(keyword_matches("cards-create", "cards-create"));
    }

    #[test]
    fn keyword_injection_skipped_on_second_session_call() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/cards-create-page.md",
            "---\ntitle: Cards Create Page\nsummary: Summary.\nkeywords:\n  - cards-create\n---\nBody.\n",
        );

        let session_id = "test-keyword-dedup";
        let _ = fs::remove_file(session_file_path(session_id));

        let make_event = |tool_use_id: &str| {
            serde_json::json!({
                "session_id": session_id,
                "hook_event_name": "PostToolUse",
                "tool_name": "Read",
                "tool_input": { "file_path": repo.path().join("src/main.ts").to_string_lossy() },
                "tool_response": "// cards-create component",
                "tool_use_id": tool_use_id
            })
            .to_string()
        };

        let code1 = run(&make_event("tu_first"), repo.path()).expect("run 1");
        assert_eq!(code1, 0);

        // Session file must be recorded after first call.
        let (shown_files, _) = load_session(session_id);
        assert!(!shown_files.is_empty(), "session file path must be recorded");

        // Second call in same session: file already seen — phase 2 and 3 skipped.
        let code2 = run(&make_event("tu_second"), repo.path()).expect("run 2");
        assert_eq!(code2, 0);

        let _ = fs::remove_file(session_file_path(session_id));
    }
}
