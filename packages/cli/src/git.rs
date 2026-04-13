use std::path::{Path, PathBuf};
use std::process::Command;

use gix::bstr::{BStr, ByteSlice};
use miette::{IntoDiagnostic, Result, WrapErr, miette};

/// A single commit from `git log --oneline`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
}

/// Git-side accelerator configuration that can improve status and inventory
/// queries without changing correctness semantics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitAccelerationState {
    pub untracked_cache: Option<bool>,
    pub split_index: Option<bool>,
}

/// Run a `git` command with the given args, rooted at `cwd`, and return stdout
/// as a UTF-8 string.  Fails with a descriptive error when the process exits
/// non-zero or produces invalid UTF-8.
fn git_output_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to spawn `git {}`", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(miette!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            stderr
        ));
    }

    Ok(output.stdout)
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(git_output_bytes(cwd, args)?)
        .into_diagnostic()
        .wrap_err("git output is not valid UTF-8")
}

fn open_repo(repo: &Path) -> Result<gix::Repository> {
    gix::open(repo)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to open git repository at '{}'", repo.display()))
}

fn utf8_repo_path(path: &BStr, context: &str) -> Result<String> {
    String::from_utf8(path.as_bytes().to_vec())
        .into_diagnostic()
        .wrap_err_with(|| context.to_owned())
}

fn for_each_tracked_path(
    repo: &gix::Repository,
    mut visit: impl FnMut(&BStr) -> Result<()>,
) -> Result<()> {
    let index = repo
        .index_or_load_from_head_or_empty()
        .into_diagnostic()
        .wrap_err("failed to load git index for tracked-path iteration")?;

    match &index {
        gix::worktree::IndexPersistedOrInMemory::Persisted(index) => {
            for entry in index.entries() {
                visit(entry.path(index))?;
            }
        }
        gix::worktree::IndexPersistedOrInMemory::InMemory(index) => {
            for entry in index.entries() {
                visit(entry.path(index))?;
            }
        }
    }

    Ok(())
}

fn status_platform(
    repo: &gix::Repository,
) -> Result<gix::status::Platform<'_, gix::progress::Discard>> {
    repo.status(gix::progress::Discard)
        .into_diagnostic()
        .wrap_err("failed to initialize git status platform")
}

fn collect_status_items(
    repo: &gix::Repository,
    include_untracked: bool,
) -> Result<Vec<gix::status::index_worktree::Item>> {
    let mut status = status_platform(repo)?;
    if include_untracked {
        status = status.untracked_files(gix::status::UntrackedFiles::Files);
    } else {
        status = status.untracked_files(gix::status::UntrackedFiles::None);
        status = status.index_worktree_options_mut(|opts| {
            opts.dirwalk_options = None;
        });
    }

    let iter = status
        .into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
        .into_diagnostic()
        .wrap_err("failed to iterate git status")?;

    let mut items = Vec::new();
    for item in iter {
        items.push(
            item.into_diagnostic()
                .wrap_err("git status iteration failed")?,
        );
    }

    Ok(items)
}

/// Return the absolute path to the repository root containing the current
/// working directory.
pub fn repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()
        .into_diagnostic()
        .wrap_err("failed to read current working directory")?;
    let repo = gix::discover(cwd)
        .into_diagnostic()
        .wrap_err("failed to discover git repository from the current directory")?;
    Ok(repo.workdir().unwrap_or(repo.path()).to_path_buf())
}

/// Resolve a git ref name (branch, tag, `HEAD`, or SHA) to a full commit SHA.
pub fn resolve_ref(repo: &Path, ref_name: &str) -> Result<String> {
    let repo = open_repo(repo)?;
    let spec = repo
        .rev_parse_single(ref_name)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to resolve ref '{ref_name}'"))?;
    Ok(spec.detach().to_string())
}

/// Return the full SHA for `HEAD`.
pub fn head_sha(repo: &Path) -> Result<String> {
    let repo = open_repo(repo)?;
    let mut head = repo
        .head()
        .into_diagnostic()
        .wrap_err("failed to read HEAD")?;
    let id = head
        .try_peel_to_id()
        .into_diagnostic()
        .wrap_err("failed to peel HEAD to an object id")?
        .ok_or_else(|| miette!("HEAD is unborn"))?;
    Ok(id.to_string())
}

/// Return true when the index contains any tracked files at all.
pub fn has_tracked_files(repo: &Path) -> Result<bool> {
    let repo = open_repo(repo)?;
    let index = repo
        .index_or_load_from_head_or_empty()
        .into_diagnostic()
        .wrap_err("failed to load git index")?;
    Ok(match &index {
        gix::worktree::IndexPersistedOrInMemory::Persisted(index) => !index.entries().is_empty(),
        gix::worktree::IndexPersistedOrInMemory::InMemory(index) => !index.entries().is_empty(),
    })
}

/// Return the current Git accelerator configuration that affects inventory and
/// status queries without requiring daemon features.
pub fn git_acceleration_state(repo: &Path) -> Result<GitAccelerationState> {
    let repo = open_repo(repo)?;
    let config = repo.config_snapshot();
    Ok(GitAccelerationState {
        untracked_cache: config.boolean("core.untrackedCache"),
        split_index: config.boolean("core.splitIndex"),
    })
}

/// Return true when the working tree has unstaged tracked changes.
pub fn has_unstaged_changes(repo: &Path) -> Result<bool> {
    let repo = open_repo(repo)?;
    Ok(collect_status_items(&repo, false)?
        .into_iter()
        .any(|item| item.summary().is_some()))
}

/// Return true when the index has staged tracked changes relative to `HEAD`.
pub fn has_staged_changes(repo: &Path) -> Result<bool> {
    let repo = open_repo(repo)?;
    let index = repo
        .index_or_load_from_head_or_empty()
        .into_diagnostic()
        .wrap_err("failed to load git index for staged-change probe")?;
    let mut staged_changes = false;

    repo.tree_index_status(
        repo.head_tree_id_or_empty()
            .into_diagnostic()
            .wrap_err("failed to resolve HEAD tree for staged-change probe")?
            .as_ref(),
        match &index {
            gix::worktree::IndexPersistedOrInMemory::Persisted(index) => index,
            gix::worktree::IndexPersistedOrInMemory::InMemory(index) => index,
        },
        None,
        gix::status::tree_index::TrackRenames::Disabled,
        |_, _, _| {
            staged_changes = true;
            Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Break(()))
        },
    )
    .into_diagnostic()
    .wrap_err("failed to compare HEAD tree to index")?;

    Ok(staged_changes)
}

/// Return tracked paths changed between two commits.
pub fn changed_paths_between(repo: &Path, from_ref: &str, to_ref: &str) -> Result<Vec<String>> {
    let range = format!("{from_ref}..{to_ref}");
    let out = git_output(repo, &["diff", "--name-only", &range, "--"])
        .wrap_err_with(|| format!("failed to list changed paths for '{range}'"))?;
    Ok(parse_line_paths(&out))
}

/// Return working-tree paths with modified, deleted, staged, or untracked
/// changes using `git status --short`.
pub fn working_tree_changed_paths(repo: &Path) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    let repo = open_repo(repo)?;

    for item in collect_status_items(&repo, true)? {
        if item.summary().is_none() {
            continue;
        }
        paths.push(utf8_repo_path(
            item.rela_path(),
            "git status path is not valid UTF-8",
        )?);
    }

    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Return the tracked and untracked, non-ignored repository file inventory as
/// repo-relative UTF-8 paths.
pub fn repo_inventory(repo: &Path) -> Result<Vec<String>> {
    let repo = open_repo(repo)?;
    let mut paths = Vec::new();

    for_each_tracked_path(&repo, |path| {
        paths.push(utf8_repo_path(path, "git inventory path is not valid UTF-8")?);
        Ok(())
    })?;

    for item in collect_status_items(&repo, true)? {
        if item.summary() == Some(gix::status::index_worktree::iter::Summary::Added) {
            paths.push(utf8_repo_path(
                item.rela_path(),
                "git inventory path is not valid UTF-8",
            )?);
        }
    }

    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Return untracked, non-ignored repository file inventory as repo-relative
/// UTF-8 paths.
pub fn untracked_paths(repo: &Path) -> Result<Vec<String>> {
    let repo = open_repo(repo)?;
    let mut paths = collect_status_items(&repo, true)?
        .into_iter()
        .filter(|item| item.summary() == Some(gix::status::index_worktree::iter::Summary::Added))
        .map(|item| utf8_repo_path(item.rela_path(), "git untracked path is not valid UTF-8"))
        .collect::<Result<Vec<_>>>()?;

    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn parse_line_paths(out: &str) -> Vec<String> {
    let mut paths = out
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Return the abbreviated SHA of the most recent commit that touched `path`
/// at or before `ref_name`.
pub fn latest_commit(repo: &Path, ref_name: &str, path: &Path) -> Result<String> {
    let path_str = path.to_string_lossy();
    let out = git_output(
        repo,
        &[
            "log",
            "-1",
            "--format=%h",
            ref_name,
            "--",
            path_str.as_ref(),
        ],
    )
    .wrap_err_with(|| {
        format!(
            "failed to find latest commit for '{}' at '{ref_name}'",
            path.display()
        )
    })?;
    let sha = out.trim().to_owned();
    if sha.is_empty() {
        return Err(miette!(
            "no commits found for '{}' at ref '{ref_name}'",
            path.display()
        ));
    }
    Ok(sha)
}

/// Return all commits reachable from HEAD but not from `sha` that touched
/// `path`, in reverse-chronological order.
pub fn commits_since(repo: &Path, sha: &str, path: &Path) -> Result<Vec<CommitInfo>> {
    let path_str = path.to_string_lossy();
    let range = format!("{sha}..HEAD");
    let out = git_output(repo, &["log", "--oneline", &range, "--", path_str.as_ref()])
        .wrap_err_with(|| {
            format!(
                "failed to list commits since '{sha}' for '{}'",
                path.display()
            )
        })?;

    let commits = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            // `--oneline` format: "<short-sha> <message>"
            let (short_sha, msg) = line.split_once(' ').unwrap_or((line, ""));
            CommitInfo {
                sha: short_sha.to_owned(),
                message: msg.to_owned(),
            }
        })
        .collect();

    Ok(commits)
}

/// Return the UTF-8 content of `path` as it existed at commit `sha`.
pub fn file_at_ref(repo: &Path, sha: &str, path: &Path) -> Result<String> {
    let path_str = path.to_string_lossy();
    let blob_spec = format!("{sha}:{}", path_str);
    git_output(repo, &["show", &blob_spec])
        .wrap_err_with(|| format!("failed to retrieve '{}' at commit '{sha}'", path.display()))
}

/// Return the `git diff --stat` summary between `sha` and HEAD for `path`.
pub fn diff_stat(repo: &Path, sha: &str, path: &Path) -> Result<String> {
    let path_str = path.to_string_lossy();
    let range = format!("{sha}..HEAD");
    git_output(repo, &["diff", "--stat", &range, "--", path_str.as_ref()]).wrap_err_with(|| {
        format!(
            "failed to compute diff stat for '{}' since '{sha}'",
            path.display()
        )
    })
}

/// Return the full unified diff between `sha` and HEAD for `path`.
pub fn diff_patch(repo: &Path, sha: &str, path: &Path) -> Result<String> {
    let path_str = path.to_string_lossy();
    let range = format!("{sha}..HEAD");
    git_output(repo, &["diff", &range, "--", path_str.as_ref()]).wrap_err_with(|| {
        format!(
            "failed to compute diff patch for '{}' since '{sha}'",
            path.display()
        )
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    /// Isolated git repository for use in tests.
    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        /// Create a new empty git repository in a temporary directory.
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let repo = TestRepo { dir };

            repo.git(&["init"]);
            // Ensure a stable default branch name regardless of system config.
            repo.git(&["checkout", "-b", "main"]);
            repo
        }

        /// Return the path to the repository root.
        fn path(&self) -> &Path {
            self.dir.path()
        }

        /// Write `content` to `path` (relative to the repo root), creating
        /// parent directories as needed.
        fn create_file(&self, path: &str, content: &str) {
            let full = self.dir.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create_dir_all");
            }
            fs::write(&full, content).expect("write file");
        }

        /// Stage all changes and create a commit with `message`.
        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-m", message]);
        }

        /// Run a git subcommand inside the repo with deterministic author/committer
        /// identity so that tests do not depend on the host's global git config.
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

    // ── repo_root ─────────────────────────────────────────────────────────────

    #[test]
    fn repo_root_returns_correct_path() {
        let repo = TestRepo::new();
        repo.create_file("file.txt", "hello");
        repo.commit("initial");

        // Call repo_root() from inside the repo dir.
        let original_dir = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(repo.path()).expect("chdir");
        let root = repo_root().expect("repo_root");
        std::env::set_current_dir(original_dir).expect("restore cwd");

        // Both paths may use different representations of the same location
        // (symlinks on macOS), so canonicalize for comparison.
        assert_eq!(
            root.canonicalize().expect("canonicalize root"),
            repo.path().canonicalize().expect("canonicalize expected")
        );
    }

    // ── resolve_ref ───────────────────────────────────────────────────────────

    #[test]
    fn resolve_ref_head() {
        let repo = TestRepo::new();
        repo.create_file("a.txt", "a");
        repo.commit("first");

        let sha = resolve_ref(repo.path(), "HEAD").expect("resolve HEAD");
        assert_eq!(sha.len(), 40, "expected full SHA, got '{sha}'");
    }

    #[test]
    fn resolve_ref_specific_commit() {
        let repo = TestRepo::new();
        repo.create_file("a.txt", "a");
        repo.commit("first");

        let head = resolve_ref(repo.path(), "HEAD").expect("resolve HEAD");
        let resolved = resolve_ref(repo.path(), &head).expect("resolve SHA");
        assert_eq!(resolved, head);
    }

    #[test]
    fn resolve_ref_nonexistent_fails() {
        let repo = TestRepo::new();
        repo.create_file("a.txt", "a");
        repo.commit("first");

        assert!(
            resolve_ref(repo.path(), "refs/heads/does-not-exist").is_err(),
            "expected error for missing ref"
        );
    }

    // ── latest_commit ─────────────────────────────────────────────────────────

    #[test]
    fn latest_commit_finds_touching_commit() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1");
        repo.commit("add doc");

        let sha = latest_commit(repo.path(), "HEAD", Path::new("doc.md")).expect("latest_commit");
        assert!(!sha.is_empty());
    }

    #[test]
    fn latest_commit_ignores_unrelated_commits() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1");
        repo.commit("add doc");

        let first_sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");

        repo.create_file("other.md", "other");
        repo.commit("add other");

        // latest_commit for doc.md should still be the first commit.
        let sha = latest_commit(repo.path(), "HEAD", Path::new("doc.md")).expect("latest_commit");
        // The short SHA should be a prefix of the first full SHA.
        assert!(
            first_sha.starts_with(&sha),
            "expected first commit sha prefix '{sha}' to be a prefix of '{first_sha}'"
        );
    }

    #[test]
    fn latest_commit_missing_file_fails() {
        let repo = TestRepo::new();
        repo.create_file("a.txt", "a");
        repo.commit("first");

        assert!(
            latest_commit(repo.path(), "HEAD", Path::new("no-such-file.md")).is_err(),
            "expected error for file with no commits"
        );
    }

    // ── commits_since ─────────────────────────────────────────────────────────

    #[test]
    fn commits_since_lists_new_commits() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1");
        repo.commit("first");

        let first_sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");

        repo.create_file("doc.md", "v2");
        repo.commit("second");

        repo.create_file("doc.md", "v3");
        repo.commit("third");

        let commits =
            commits_since(repo.path(), &first_sha, Path::new("doc.md")).expect("commits_since");

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].message, "third");
        assert_eq!(commits[1].message, "second");
    }

    #[test]
    fn commits_since_empty_when_no_new_commits() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1");
        repo.commit("only");

        let sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");
        let commits = commits_since(repo.path(), &sha, Path::new("doc.md")).expect("commits_since");

        assert!(commits.is_empty());
    }

    // ── file_at_ref ───────────────────────────────────────────────────────────

    #[test]
    fn file_at_ref_returns_content() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "hello world");
        repo.commit("add doc");

        let sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");
        let content = file_at_ref(repo.path(), &sha, Path::new("doc.md")).expect("file_at_ref");

        assert_eq!(content, "hello world");
    }

    #[test]
    fn file_at_ref_old_version() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "version one");
        repo.commit("v1");

        let sha_v1 = resolve_ref(repo.path(), "HEAD").expect("HEAD");

        repo.create_file("doc.md", "version two");
        repo.commit("v2");

        let content = file_at_ref(repo.path(), &sha_v1, Path::new("doc.md")).expect("file_at_ref");
        assert_eq!(content, "version one");
    }

    #[test]
    fn file_at_ref_nonexistent_fails() {
        let repo = TestRepo::new();
        repo.create_file("a.txt", "a");
        repo.commit("first");

        let sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");
        assert!(
            file_at_ref(repo.path(), &sha, Path::new("missing.md")).is_err(),
            "expected error for missing file"
        );
    }

    // ── diff_stat ─────────────────────────────────────────────────────────────

    #[test]
    fn diff_stat_returns_nonempty_for_changed_file() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "line one\n");
        repo.commit("initial");

        let sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");

        repo.create_file("doc.md", "line one\nline two\n");
        repo.commit("add line");

        let stat = diff_stat(repo.path(), &sha, Path::new("doc.md")).expect("diff_stat");
        assert!(!stat.trim().is_empty(), "diff --stat should not be empty");
        assert!(stat.contains("doc.md"));
    }

    // ── diff_patch ────────────────────────────────────────────────────────────

    #[test]
    fn diff_patch_returns_nonempty_for_changed_file() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "line one\n");
        repo.commit("initial");

        let sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");

        repo.create_file("doc.md", "line one\nline two\n");
        repo.commit("add line");

        let patch = diff_patch(repo.path(), &sha, Path::new("doc.md")).expect("diff_patch");
        assert!(!patch.trim().is_empty(), "diff patch should not be empty");
        assert!(patch.contains("+line two"));
    }

    #[test]
    fn diff_patch_empty_for_unchanged_file() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "unchanged\n");
        repo.commit("initial");

        let sha = resolve_ref(repo.path(), "HEAD").expect("HEAD");

        let patch = diff_patch(repo.path(), &sha, Path::new("doc.md")).expect("diff_patch");
        assert!(
            patch.trim().is_empty(),
            "diff patch should be empty when nothing changed"
        );
    }

    #[test]
    fn has_tracked_files_false_for_empty_repo() {
        let repo = TestRepo::new();
        assert!(!has_tracked_files(repo.path()).expect("tracked files probe"));
    }

    #[test]
    fn has_tracked_files_true_for_staged_file_without_head() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "hello\n");
        repo.git(&["add", "-A"]);
        assert!(has_tracked_files(repo.path()).expect("tracked files probe"));
    }

    #[test]
    fn head_sha_fails_for_unborn_head() {
        let repo = TestRepo::new();
        assert!(head_sha(repo.path()).is_err(), "expected unborn HEAD to fail");
    }

    #[test]
    fn git_acceleration_state_reads_optional_config() {
        let repo = TestRepo::new();
        repo.git(&["config", "core.untrackedCache", "true"]);
        repo.git(&["config", "core.splitIndex", "false"]);

        let state = git_acceleration_state(repo.path()).expect("git acceleration state");
        assert_eq!(state.untracked_cache, Some(true));
        assert_eq!(state.split_index, Some(false));
    }

    #[test]
    fn repo_inventory_includes_tracked_and_untracked_paths() {
        let repo = TestRepo::new();
        repo.create_file("tracked.md", "tracked\n");
        repo.create_file("notes/untracked.md", "untracked\n");
        repo.git(&["add", "tracked.md"]);

        let inventory = repo_inventory(repo.path()).expect("repo inventory");
        assert_eq!(
            inventory,
            vec!["notes/untracked.md".to_owned(), "tracked.md".to_owned()]
        );
    }

    #[test]
    fn status_probes_detect_staged_and_unstaged_changes_in_process() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1\n");
        repo.commit("initial");

        repo.create_file("doc.md", "v2\n");
        assert!(
            has_unstaged_changes(repo.path()).expect("unstaged probe"),
            "expected unstaged modification"
        );
        assert_eq!(
            working_tree_changed_paths(repo.path()).expect("working tree changed paths"),
            vec!["doc.md".to_owned()]
        );

        repo.git(&["add", "doc.md"]);
        assert!(
            has_staged_changes(repo.path()).expect("staged probe"),
            "expected staged modification"
        );
        assert_eq!(
            untracked_paths(repo.path()).expect("untracked paths"),
            Vec::<String>::new()
        );
    }
}
