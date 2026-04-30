//! Pre-flight check: which anchored paths are absent from `HEAD`?
//!
//! Runs `git ls-tree -r --name-only HEAD` exactly once, builds a `HashSet`,
//! and diffs the caller's anchored paths against it. Failure to invoke git
//! degrades to `Skipped { reason }` — the scaffold itself must keep running.

#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreflightResult {
    /// Pre-flight ran. `missing` may be empty (everything is in HEAD).
    Ok { missing: Vec<String> },
    /// Pre-flight could not run; `reason` is rendered into the script header
    /// as `# Pre-flight skipped: <reason>` so the operator decides.
    Skipped { reason: String },
}

/// Diff `anchored_paths` against the set of paths in `HEAD`. Runs git once.
pub(crate) fn missing_in_head(repo_root: &Path, anchored_paths: &[String]) -> PreflightResult {
    let output = match Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .current_dir(repo_root)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return PreflightResult::Skipped {
                reason: format!("git invocation failed: {e}"),
            };
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        let reason = if trimmed.is_empty() {
            format!("git ls-tree exited with status {}", output.status)
        } else {
            trimmed.to_string()
        };
        return PreflightResult::Skipped { reason };
    }
    let in_head = parse_ls_tree(&output.stdout);
    let missing: Vec<String> = anchored_paths
        .iter()
        .filter(|p| !in_head.contains(p.as_str()))
        .cloned()
        .collect();
    PreflightResult::Ok { missing }
}

/// Parse `git ls-tree -r --name-only HEAD` output (LF-terminated paths) into
/// a set. Empty lines are dropped.
fn parse_ls_tree(bytes: &[u8]) -> HashSet<String> {
    let s = String::from_utf8_lossy(bytes);
    s.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ls_tree_handles_trailing_newline() {
        let out = b"src/a.rs\nsrc/b.rs\nwiki/foo.md\n";
        let set = parse_ls_tree(out);
        assert!(set.contains("src/a.rs"));
        assert!(set.contains("src/b.rs"));
        assert!(set.contains("wiki/foo.md"));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn parse_ls_tree_skips_empty_lines() {
        let out = b"\nsrc/a.rs\n\n\n";
        let set = parse_ls_tree(out);
        assert_eq!(set.len(), 1);
        assert!(set.contains("src/a.rs"));
    }

    #[test]
    fn diff_logic_via_helper_finds_missing_paths() {
        // Pure-logic test of the diff using a captured ls-tree byte string.
        let ls_tree = b"src/a.rs\nsrc/b.rs\n";
        let in_head = parse_ls_tree(ls_tree);
        let anchored = vec![
            "src/a.rs".to_string(),
            "src/missing.rs".to_string(),
            "src/b.rs".to_string(),
        ];
        let missing: Vec<String> = anchored
            .into_iter()
            .filter(|p| !in_head.contains(p.as_str()))
            .collect();
        assert_eq!(missing, vec!["src/missing.rs".to_string()]);
    }

    #[test]
    fn missing_in_head_in_non_repo_returns_skipped() {
        // /tmp is (very likely) not a git repo. We just want to confirm we
        // don't panic and we surface a Skipped variant.
        let tmp = std::env::temp_dir();
        let result = missing_in_head(&tmp, &["any/path".to_string()]);
        // Either Skipped (no repo) or Ok if /tmp happens to be a git tree.
        // The point is: no panic.
        match result {
            PreflightResult::Skipped { reason } => assert!(!reason.is_empty()),
            PreflightResult::Ok { .. } => {}
        }
    }
}
