use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Loaded from `.wiki/.wikiignore`. Anchored at `repo_root` so patterns like
/// `drafts/` match `repo_root/drafts/`, not `.wiki/drafts/`.
pub struct WikiIgnore {
    inner: Gitignore,
}

impl WikiIgnore {
    pub fn load(repo_root: &Path) -> anyhow::Result<Self> {
        let path = repo_root.join(".wiki").join(".wikiignore");
        if !path.exists() {
            return Ok(Self {
                inner: Gitignore::empty(),
            });
        }
        // Read all content first so IO errors are reported immediately (fail closed).
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read .wiki/.wikiignore: {e}"))?;
        let mut builder = GitignoreBuilder::new(repo_root); // root = repo_root
                                                            // add_line(None, line) anchors each pattern to repo_root and returns Err
                                                            // on any malformed pattern — fail closed on bad lines, not silently skipped.
                                                            // A malformed pattern that was meant to hide sensitive drafts must not be
                                                            // silently dropped, leaving those files visible.
        for line in content.lines() {
            builder
                .add_line(None, line)
                .map_err(|e| anyhow::anyhow!("invalid pattern in .wiki/.wikiignore: {e}"))?;
        }
        let inner = builder.build().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self { inner })
    }

    /// `true` if `repo_relative_path` should be hidden from wiki surfaces.
    ///
    /// Matches git's gitignore(5) rule that "it is not possible to re-include a
    /// file if a parent directory of that file is excluded." `ignore`'s
    /// `matched_path_or_any_parents` evaluates a `!`-whitelist against the literal
    /// full path and returns `Whitelist` BEFORE walking parents — so `drafts/`
    /// plus `!drafts/page.md` would re-include (expose) `page.md`. That is
    /// fail-open and diverges from git in the unsafe direction.
    ///
    /// We instead walk each ancestor directory first: if any ancestor directory
    /// is itself ignored (and not re-included at that ancestor level), the path
    /// stays ignored and a deeper whitelist cannot resurrect it. Only when no
    /// ancestor directory is excluded do we honor a same-level/sibling negation
    /// (e.g. `*.tmp` + `!keep.tmp`), exactly as git does.
    pub fn is_ignored(&self, repo_relative_path: &Path) -> bool {
        // First: if any ancestor directory is excluded, the path is ignored and
        // no deeper `!` whitelist may re-include it (git's no-re-include rule).
        let mut ancestor = std::path::PathBuf::new();
        let mut components = repo_relative_path.components().peekable();
        while let Some(component) = components.next() {
            // Skip the final component — that is the path itself, handled below.
            if components.peek().is_none() {
                break;
            }
            ancestor.push(component);
            if self.inner.matched(&ancestor, true).is_ignore() {
                return true;
            }
        }

        // No ancestor directory is excluded: honor a same-level decision,
        // including a sibling-level `!` re-include, matching git.
        self.inner
            .matched_path_or_any_parents(repo_relative_path, false)
            .is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_ignore(root: &Path, content: &str) {
        let dir = root.join(".wiki");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".wikiignore"), content).unwrap();
    }

    #[test]
    fn test_absent_file_is_noop() {
        let tmp = TempDir::new().unwrap();
        let wi = WikiIgnore::load(tmp.path()).unwrap();
        assert!(!wi.is_ignored(Path::new("anything.md")));
        assert!(!wi.is_ignored(Path::new("drafts/page.md")));
    }

    #[test]
    fn test_dir_pattern_matches_subtree() {
        let tmp = TempDir::new().unwrap();
        write_ignore(tmp.path(), "drafts/\n");
        let wi = WikiIgnore::load(tmp.path()).unwrap();
        assert!(wi.is_ignored(Path::new("drafts/page.md")));
        assert!(wi.is_ignored(Path::new("sub/drafts/page.md")));
        assert!(!wi.is_ignored(Path::new("draftstore.md")));
    }

    #[test]
    fn test_anchored_pattern() {
        let tmp = TempDir::new().unwrap();
        write_ignore(tmp.path(), "/secret.md\n");
        let wi = WikiIgnore::load(tmp.path()).unwrap();
        assert!(wi.is_ignored(Path::new("secret.md")));
        assert!(!wi.is_ignored(Path::new("sub/secret.md")));
    }

    #[test]
    fn test_no_reinclude_under_excluded_directory() {
        // git's gitignore(5) rule: "it is not possible to re-include a file if
        // a parent directory of that file is excluded." Once `drafts/` is
        // excluded, the deeper `!drafts/page.md` whitelist cannot resurrect the
        // file. Verified identical to `git check-ignore`.
        let tmp = TempDir::new().unwrap();
        write_ignore(tmp.path(), "drafts/\n!drafts/page.md\n");
        let wi = WikiIgnore::load(tmp.path()).unwrap();
        assert!(wi.is_ignored(Path::new("drafts/page.md")));
        assert!(wi.is_ignored(Path::new("drafts/other.md")));
    }

    #[test]
    fn test_no_reinclude_deep_under_excluded_directory() {
        // A whitelist nested several levels below an excluded directory is also
        // powerless, matching git.
        let tmp = TempDir::new().unwrap();
        write_ignore(tmp.path(), "drafts/\n!drafts/sub/deep.md\n");
        let wi = WikiIgnore::load(tmp.path()).unwrap();
        assert!(wi.is_ignored(Path::new("drafts/sub/deep.md")));
    }

    #[test]
    fn test_sibling_negation_reincludes() {
        // No ancestor directory is excluded, so a same-level `!` re-include is
        // honored — exactly as git does for `*.tmp` + `!keep.tmp`.
        let tmp = TempDir::new().unwrap();
        write_ignore(tmp.path(), "*.tmp\n!keep.tmp\n");
        let wi = WikiIgnore::load(tmp.path()).unwrap();
        assert!(wi.is_ignored(Path::new("scratch.tmp")));
        assert!(!wi.is_ignored(Path::new("keep.tmp")));
    }

    #[test]
    fn test_empty_and_comment_only_no_matches() {
        let tmp = TempDir::new().unwrap();
        write_ignore(tmp.path(), "");
        let wi = WikiIgnore::load(tmp.path()).unwrap();
        assert!(!wi.is_ignored(Path::new("x.md")));

        let tmp2 = TempDir::new().unwrap();
        write_ignore(tmp2.path(), "# just a comment\n\n# another\n");
        let wi2 = WikiIgnore::load(tmp2.path()).unwrap();
        assert!(!wi2.is_ignored(Path::new("x.md")));
    }

    #[test]
    fn test_malformed_pattern_fails_closed() {
        let tmp = TempDir::new().unwrap();
        // A reversed character-class range `[z-a]` is an invalid glob; the
        // per-line `add_line` parse must propagate the error (fail closed)
        // rather than silently skipping the line.
        write_ignore(tmp.path(), "[z-a]\n");
        let res = WikiIgnore::load(tmp.path());
        assert!(res.is_err());
    }
}
