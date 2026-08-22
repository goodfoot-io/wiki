//! Pre-existing legacy helpers left over from the deleted Turso-era `index.rs`.
//! They remain part of the lib surface (used by integration tests) but are
//! unused inside the bin crate. Suppress bin-crate dead_code without
//! affecting the lib build.
#![allow(dead_code)]

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gix::bstr::{BStr, ByteSlice};
use miette::{IntoDiagnostic, Result, WrapErr, miette};

use crate::index::DocSource;

/// Git-side accelerator configuration that can improve status and inventory
/// queries without changing correctness semantics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitAccelerationState {
    pub untracked_cache: Option<bool>,
    pub split_index: Option<bool>,
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

/// Open the persisted git index (`.git/index`) and fail closed when it is
/// absent.  Used by `--source=index` code paths so a missing index never
/// silently degrades to a HEAD-derived in-memory index.
fn open_persisted_index(repo: &gix::Repository) -> Result<gix::worktree::Index> {
    repo.try_index()
        .into_diagnostic()
        .wrap_err("failed to open git index")?
        .ok_or_else(|| miette!("git index is absent — nothing staged"))
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

/// Return the absolute, lexically normalized path of the repository's common
/// git directory (`.git` of the main repository). From a linked worktree
/// this is the *main* repo's `.git`, so every worktree of one repo shares
/// one anchor cache under it (plan decision 2).
///
/// Discovery honors `GIT_DIR` environment overrides so a bogus `GIT_DIR`
/// fails the same way git would — fail closed to "cache disabled for the
/// run". The returned path is normalized because a linked worktree's
/// `common_dir()` carries `..` segments (spike S2). A discovery failure is
/// `Err`; the caller disables the anchor cache for the run (uncached
/// computation is always correct).
pub fn common_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()
        .into_diagnostic()
        .wrap_err("failed to read current working directory")?;
    let repo = gix::discover_with_environment_overrides(cwd)
        .into_diagnostic()
        .wrap_err("failed to discover git repository from the current directory")?;
    Ok(normalize_lexically(repo.common_dir().to_path_buf()))
}

/// Collapse `.`/`..` segments lexically, preserving the leading root.
///
/// A linked worktree's `common_dir()` carries `..` segments — gix returns
/// `…/.git/worktrees/N/../..` where git's `rev-parse --git-common-dir`
/// reports `…/.git` (spike S2). The collapsed path is the same directory,
/// and equality against `rev-parse` output and path joins under it need the
/// normalized form.
fn normalize_lexically(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
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

/// Return the tracked and untracked, non-ignored repository file inventory as
/// repo-relative UTF-8 paths.
///
/// Tracked paths are sourced from the git index; untracked-but-not-ignored
/// paths are sourced from a `ignore::WalkBuilder` walk that honours
/// `.gitignore`, `.git/info/exclude`, and `core.excludesFile` (the same set
/// of sources `git status --short` consults).
pub fn repo_inventory(repo: &Path) -> Result<Vec<String>> {
    let gix_repo = open_repo(repo)?;
    let mut paths: HashSet<String> = HashSet::new();

    for_each_tracked_path(&gix_repo, |path| {
        paths.insert(utf8_repo_path(
            path,
            "git inventory path is not valid UTF-8",
        )?);
        Ok(())
    })?;

    // Parallelize the worktree walk so blocked filesystem stats overlap on a
    // hostile (fuseblk) filesystem. Every builder setting matches the serial
    // walk; the parallel visitor replicates the exact filter logic.
    let collected: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    ignore::WalkBuilder::new(repo)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .build_parallel()
        .run(|| {
            Box::new(|result| {
                // The old `.flatten()` silently dropped errors — skip on error.
                let Ok(entry) = result else {
                    return ignore::WalkState::Continue;
                };
                let entry_path = entry.path();
                if entry_path == repo {
                    return ignore::WalkState::Continue;
                }
                // Skip directories: we want files only. The walker yields both.
                if entry.file_type().is_some_and(|t| t.is_dir()) {
                    return ignore::WalkState::Continue;
                }
                // Skip `.git` and anything inside it.
                if let Ok(rel) = entry_path.strip_prefix(repo) {
                    if rel.components().next().map(|c| c.as_os_str())
                        == Some(std::ffi::OsStr::new(".git"))
                    {
                        return ignore::WalkState::Continue;
                    }
                    collected
                        .lock()
                        .expect("repo_inventory walk mutex poisoned")
                        .push(rel.to_string_lossy().into_owned());
                }
                ignore::WalkState::Continue
            })
        });
    for p in collected.into_inner().expect("repo_inventory walk mutex poisoned") {
        paths.insert(p);
    }

    let mut paths: Vec<String> = paths.into_iter().collect();
    paths.sort();
    Ok(paths)
}

/// Return the subset of `candidates` (repo-relative paths) that git treats as
/// **ignored** — matched by any gitignore source (`.gitignore`,
/// `.git/info/exclude`, `core.excludesFile`). Resolution is delegated to
/// `git check-ignore --stdin` so nested ignore files, negations, and config
/// excludes are honoured exactly as git itself would.
///
/// `git check-ignore` exits 0 when one or more paths are ignored and **1 when
/// none are** — the latter is a normal "nothing ignored" result, not a failure,
/// so it is mapped to an empty set. Any other exit (e.g. 128) is a genuine
/// error and is propagated.
///
/// A path being ignored is distinct from it being untracked: an
/// untracked-but-not-ignored file is a legitimate anchor target that resolves
/// once committed, whereas an ignored path (a build artifact) is never
/// committed and can never resolve. Only the latter is reported here.
pub fn ignored_paths(repo: &Path, candidates: &[String]) -> Result<HashSet<String>> {
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }

    let mut child = Command::new("git")
        .current_dir(repo)
        .args(["check-ignore", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .into_diagnostic()
        .wrap_err("failed to spawn `git check-ignore --stdin`")?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| miette!("failed to open stdin for `git check-ignore`"))?;
        for path in candidates {
            writeln!(stdin, "{path}")
                .into_diagnostic()
                .wrap_err("failed to write path to `git check-ignore` stdin")?;
        }
        // `stdin` drops here, closing the pipe so check-ignore can finish.
    }

    let output = child
        .wait_with_output()
        .into_diagnostic()
        .wrap_err("failed to collect output from `git check-ignore`")?;

    // Exit 0 = some paths ignored; exit 1 = none ignored (normal). Anything
    // else is a real failure.
    match output.status.code() {
        Some(0) | Some(1) => {}
        other => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(miette!(
                "git check-ignore failed (exit {:?}): {}",
                other,
                stderr
            ));
        }
    }

    let stdout = String::from_utf8(output.stdout)
        .into_diagnostic()
        .wrap_err("git check-ignore output is not valid UTF-8")?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

// ─── Index / HEAD helpers ─────────────────────────────────────────────────────

/// Return repo-relative UTF-8 paths of all entries in the git index.
///
/// Fails closed when `.git/index` is absent — `--source=index` must never
/// silently substitute HEAD content for staged content.
pub fn index_tracked_paths(repo: &Path) -> Result<Vec<String>> {
    let repo = open_repo(repo)?;
    let index = open_persisted_index(&repo)?;
    let mut paths = Vec::new();
    for entry in index.entries() {
        paths.push(utf8_repo_path(
            entry.path(&index),
            "git index path is not valid UTF-8",
        )?);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Maximum symlink resolution depth before giving up (mirrors the small caps
/// used by most filesystems and prevents cycles in pathological repos).
const SYMLINK_RECURSION_LIMIT: usize = 8;

/// Look up the index entry for `path_rel`, returning the blob id and whether
/// it represents a symlink.
///
/// Fails closed when `.git/index` is absent — see `open_persisted_index`.
fn index_entry_id_and_kind(
    repo: &gix::Repository,
    path_rel: &str,
) -> Result<Option<(gix::ObjectId, gix::index::entry::Mode)>> {
    let index = open_persisted_index(repo)?;
    Ok(index
        .entry_by_path(gix::bstr::BStr::new(path_rel.as_bytes()))
        .map(|e| (e.id, e.mode)))
}

/// Resolve a relative symlink target against the directory containing
/// `from_path_rel`, normalising `.` and `..` segments and producing a
/// repo-relative UTF-8 path.  Returns `None` if the resolution escapes the
/// repository root.
fn resolve_symlink_target_rel(from_path_rel: &str, target: &str) -> Option<String> {
    // Absolute targets escape the repo and are unsupported under index/head
    // sources (we cannot read arbitrary worktree paths from a snapshot).
    if target.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = if let Some((dir, _)) = from_path_rel.rsplit_once('/') {
        dir.split('/').filter(|s| !s.is_empty()).collect()
    } else {
        Vec::new()
    };
    for component in target.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// Read the blob content for `path_rel` from the git index, or `None` if the
/// path is not present in the index.
///
/// Mirrors `--source=worktree`'s `fs::read_to_string` semantics by following
/// symlinks within the same source up to `SYMLINK_RECURSION_LIMIT` hops.
pub fn read_index_blob(repo: &Path, path_rel: &str) -> Result<Option<String>> {
    let gix_repo = open_repo(repo)?;
    read_index_blob_inner(&gix_repo, path_rel, 0)
}

fn read_index_blob_inner(
    repo: &gix::Repository,
    path_rel: &str,
    depth: usize,
) -> Result<Option<String>> {
    if depth > SYMLINK_RECURSION_LIMIT {
        return Err(miette!(
            "symlink resolution exceeded depth limit at index path '{path_rel}'"
        ));
    }
    let Some((id, mode)) = index_entry_id_and_kind(repo, path_rel)? else {
        return Ok(None);
    };
    let object = repo
        .find_object(id)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read blob for index entry '{path_rel}'"))?;
    if mode == gix::index::entry::Mode::SYMLINK {
        let target = String::from_utf8(object.data.to_vec())
            .into_diagnostic()
            .wrap_err_with(|| format!("symlink target for '{path_rel}' is not valid UTF-8"))?;
        let Some(resolved) = resolve_symlink_target_rel(path_rel, target.trim_end_matches('\0'))
        else {
            return Ok(None);
        };
        return read_index_blob_inner(repo, &resolved, depth + 1);
    }
    let content = String::from_utf8(object.data.to_vec())
        .into_diagnostic()
        .wrap_err_with(|| format!("blob for '{path_rel}' is not valid UTF-8"))?;
    Ok(Some(content))
}

/// Return repo-relative UTF-8 paths of all entries reachable from `HEAD`.
pub fn head_tracked_paths(repo: &Path) -> Result<Vec<String>> {
    let repo = open_repo(repo)?;
    let mut head = repo
        .head()
        .into_diagnostic()
        .wrap_err("failed to read HEAD for tree traversal")?;
    let commit_id = head
        .try_peel_to_id()
        .into_diagnostic()
        .wrap_err("failed to peel HEAD to commit id")?
        .ok_or_else(|| miette!("HEAD is unborn — no tree to traverse"))?;
    let commit = repo
        .find_object(commit_id)
        .into_diagnostic()
        .wrap_err("failed to find HEAD commit object")?
        .try_into_commit()
        .map_err(|_| miette!("HEAD object is not a commit"))?;
    let tree = commit
        .tree()
        .into_diagnostic()
        .wrap_err("failed to read tree from HEAD commit")?;

    let mut paths = Vec::new();
    for_each_tree_entry_recursive(&repo, tree.id, &mut String::new(), &mut paths)?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn for_each_tree_entry_recursive(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    prefix: &mut String,
    paths: &mut Vec<String>,
) -> Result<()> {
    let tree_obj = repo
        .find_object(tree_id)
        .into_diagnostic()
        .wrap_err("failed to find tree object")?;
    let tree = tree_obj
        .try_into_tree()
        .map_err(|_| miette!("object is not a tree"))?;

    for entry_ref in tree.iter() {
        let entry = entry_ref
            .into_diagnostic()
            .wrap_err("failed to decode tree entry")?;
        let name = std::str::from_utf8(entry.filename())
            .into_diagnostic()
            .wrap_err("tree entry name is not valid UTF-8")?;
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.mode().kind() {
            gix::object::tree::EntryKind::Tree => {
                let sub_id = entry.object_id();
                let prev_len = prefix.len();
                if prefix.is_empty() {
                    prefix.push_str(name);
                } else {
                    prefix.push('/');
                    prefix.push_str(name);
                }
                for_each_tree_entry_recursive(repo, sub_id, prefix, paths)?;
                prefix.truncate(prev_len);
            }
            gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable => {
                paths.push(path);
            }
            gix::object::tree::EntryKind::Link => {
                // Symlinks are surfaced as paths so callers can follow them
                // via `read_head_blob`, which dereferences within the same
                // source for parity with worktree `fs::read_to_string`.
                paths.push(path);
            }
            // Submodule gitlinks (`Commit`): wiki content does not live in
            // submodules; full traversal is out of scope.  Skip silently.
            gix::object::tree::EntryKind::Commit => {}
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
    Ok(())
}

/// Read the blob content for `path_rel` from the `HEAD` tree, or `None` if the
/// path is absent at HEAD.
///
/// Symlinks are followed within the same source up to
/// `SYMLINK_RECURSION_LIMIT` hops, mirroring `--source=worktree`'s
/// `fs::read_to_string` semantics.  Submodule gitlinks (`EntryKind::Commit`)
/// are reported as absent because wiki content does not live in submodules.
pub fn read_head_blob(repo: &Path, path_rel: &str) -> Result<Option<String>> {
    let gix_repo = open_repo(repo)?;
    read_head_blob_inner(&gix_repo, path_rel, 0)
}

fn head_root_tree_id(repo: &gix::Repository) -> Result<gix::ObjectId> {
    let mut head = repo
        .head()
        .into_diagnostic()
        .wrap_err("failed to read HEAD for blob read")?;
    let commit_id = head
        .try_peel_to_id()
        .into_diagnostic()
        .wrap_err("failed to peel HEAD to commit id")?
        .ok_or_else(|| miette!("HEAD is unborn"))?;
    let commit = repo
        .find_object(commit_id)
        .into_diagnostic()
        .wrap_err("failed to find HEAD commit")?
        .try_into_commit()
        .map_err(|_| miette!("HEAD is not a commit"))?;
    let tree = commit
        .tree()
        .into_diagnostic()
        .wrap_err("failed to read HEAD tree")?;
    Ok(tree.id)
}

fn read_head_blob_inner(
    repo: &gix::Repository,
    path_rel: &str,
    depth: usize,
) -> Result<Option<String>> {
    if depth > SYMLINK_RECURSION_LIMIT {
        return Err(miette!(
            "symlink resolution exceeded depth limit at HEAD path '{path_rel}'"
        ));
    }
    let root_tree_id = head_root_tree_id(repo)?;
    let parts: Vec<&str> = path_rel.split('/').collect();
    let mut current_tree_id = root_tree_id;

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let tree_obj = repo
            .find_object(current_tree_id)
            .into_diagnostic()
            .wrap_err("failed to find tree object during path walk")?;
        let tree = tree_obj
            .try_into_tree()
            .map_err(|_| miette!("object is not a tree during path walk"))?;

        let mut found = false;
        for entry_ref in tree.iter() {
            let entry = entry_ref
                .into_diagnostic()
                .wrap_err("failed to decode tree entry")?;
            let name = std::str::from_utf8(entry.filename())
                .into_diagnostic()
                .wrap_err("tree entry name is not valid UTF-8")?;
            if name != *part {
                continue;
            }
            found = true;
            let kind = entry.mode().kind();
            if is_last {
                if matches!(kind, gix::object::tree::EntryKind::Link) {
                    // Resolve symlink within the same source.
                    let object = repo
                        .find_object(entry.object_id())
                        .into_diagnostic()
                        .wrap_err_with(|| {
                            format!("failed to read symlink blob for '{path_rel}'")
                        })?;
                    let target = String::from_utf8(object.data.to_vec())
                        .into_diagnostic()
                        .wrap_err_with(|| {
                            format!("symlink target for '{path_rel}' is not valid UTF-8")
                        })?;
                    let Some(resolved) =
                        resolve_symlink_target_rel(path_rel, target.trim_end_matches('\0'))
                    else {
                        return Ok(None);
                    };
                    return read_head_blob_inner(repo, &resolved, depth + 1);
                }
                if matches!(kind, gix::object::tree::EntryKind::Commit) {
                    // Submodule gitlink — out of scope; treat as absent.
                    return Ok(None);
                }
                let object = repo
                    .find_object(entry.object_id())
                    .into_diagnostic()
                    .wrap_err_with(|| format!("failed to read blob for '{path_rel}'"))?;
                let content = String::from_utf8(object.data.to_vec())
                    .into_diagnostic()
                    .wrap_err_with(|| format!("blob '{path_rel}' is not valid UTF-8"))?;
                return Ok(Some(content));
            } else if matches!(kind, gix::object::tree::EntryKind::Tree) {
                current_tree_id = entry.object_id();
            } else {
                // Blob, symlink, or commit at a non-terminal position:
                // the path resolves through a non-directory — unresolvable.
                return Ok(None);
            }
            break;
        }
        if !found {
            return Ok(None);
        }
    }
    Ok(None)
}

/// Read the raw bytes of the blob at `commit_sha_hex`:`path_rel` (a full
/// 40-hex commit SHA) — the in-process equivalent of `git show
/// <sha>:<path>`, opening the repository first. Multi-commit history walks
/// should hold a [`GitReader`] and call
/// [`GitReader::read_blob_at_commit`] instead, so the open cost is paid
/// once.
///
/// `Ok(None)` when the path is absent at that commit, resolves through a
/// non-directory entry or a submodule gitlink, or names an unloadable blob
/// object — the full `git show` exit-128 family, whose members callers
/// disambiguate via the availability probe when it matters. A bad object id,
/// a missing commit or tree object, and other structural failures are
/// `Err`. Like `git show`, symlinks are NOT dereferenced — the link's own
/// bytes are returned.
pub fn read_blob_at_commit(
    repo_root: &Path,
    commit_sha_hex: &str,
    path_rel: &str,
) -> Result<Option<Vec<u8>>> {
    let repo = open_repo(repo_root)?;
    read_blob_at_commit_inner(&repo, commit_sha_hex, path_rel)
}

fn read_blob_at_commit_inner(
    repo: &gix::Repository,
    commit_sha_hex: &str,
    path_rel: &str,
) -> Result<Option<Vec<u8>>> {
    let id = gix::ObjectId::from_hex(commit_sha_hex.as_bytes())
        .map_err(|_| miette!("'{commit_sha_hex}' is not a full hex object id"))?;
    let commit = repo
        .find_object(id)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read object '{commit_sha_hex}'"))?
        .try_into_commit()
        .map_err(|_| miette!("object '{commit_sha_hex}' is not a commit"))?;
    let tree = commit
        .tree()
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read tree of commit '{commit_sha_hex}'"))?;
    blob_bytes_at_tree_path(repo, tree.id, path_rel)
}

/// Walk `path_rel` component-wise from `root_tree_id` to its blob and return
/// the raw bytes. A missing intermediate or terminal component, a non-tree at
/// a non-terminal position, a tree or gitlink at the terminal position are
/// all `Ok(None)` — the path holds no readable blob there.
fn blob_bytes_at_tree_path(
    repo: &gix::Repository,
    root_tree_id: gix::ObjectId,
    path_rel: &str,
) -> Result<Option<Vec<u8>>> {
    let parts: Vec<&str> = path_rel.split('/').collect();
    let mut current_tree_id = root_tree_id;

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let tree_obj = repo
            .find_object(current_tree_id)
            .into_diagnostic()
            .wrap_err("failed to find tree object during path walk")?;
        let tree = tree_obj
            .try_into_tree()
            .map_err(|_| miette!("object is not a tree during path walk"))?;

        let mut found = false;
        for entry_ref in tree.iter() {
            let entry = entry_ref
                .into_diagnostic()
                .wrap_err("failed to decode tree entry")?;
            let name = std::str::from_utf8(entry.filename())
                .into_diagnostic()
                .wrap_err("tree entry name is not valid UTF-8")?;
            if name != *part {
                continue;
            }
            found = true;
            match entry.mode().kind() {
                gix::object::tree::EntryKind::Tree => {
                    if is_last {
                        // A directory occupies the path — no blob to read.
                        return Ok(None);
                    }
                    current_tree_id = entry.object_id();
                }
                gix::object::tree::EntryKind::Commit => {
                    // Submodule gitlink — wiki content does not live in
                    // submodules; treat as absent.
                    return Ok(None);
                }
                _ => {
                    if is_last {
                        // Blob, executable blob, or symlink: return the raw
                        // bytes (`git show` does not dereference symlinks).
                        // A present-but-unloadable object (pruned, corrupt)
                        // maps to `Ok(None)` — the exact `git show` exit-128
                        // contract this helper replaces, which the drift
                        // engine's availability probe (plan decision 1)
                        // disambiguates on the cache-write path.
                        return Ok(repo
                            .find_object(entry.object_id())
                            .ok()
                            .map(|object| object.data.to_vec()));
                    }
                    // The path resolves through a non-directory.
                    return Ok(None);
                }
            }
            break;
        }
        if !found {
            return Ok(None);
        }
    }
    Ok(None)
}

/// Return `true` if `path_rel` has an entry in the git index.
///
/// Fails closed when `.git/index` is absent — see `open_persisted_index`.
pub fn has_index_entry(repo: &Path, path_rel: &str) -> Result<bool> {
    let repo = open_repo(repo)?;
    let index = open_persisted_index(&repo)?;
    Ok(index
        .entry_by_path(gix::bstr::BStr::new(path_rel.as_bytes()))
        .is_some())
}

/// Return `true` if `path_rel` exists in the `HEAD` tree.
pub fn has_head_entry(repo: &Path, path_rel: &str) -> Result<bool> {
    Ok(read_head_blob(repo, path_rel)?.is_some())
}

/// Return a liveness signal for the git index, used as the cache-revision key
/// for `DocSource::Index`.  The signal is the hex of the index file's trailing
/// SHA-1 checksum (verified by gix on load), so any change to staged content
/// produces a new signal — including same-size restages that would alias on a
/// coarse `(mtime, size)` key.
///
/// Fails closed when `.git/index` is absent: this signal is load-bearing for
/// cache correctness, and `--source=index` errors before the signal is asked
/// for in the missing-index case.
pub fn index_revision_signal(repo: &Path) -> Result<String> {
    let gix_repo = open_repo(repo)?;
    let index = open_persisted_index(&gix_repo)?;
    let checksum = index.checksum().ok_or_else(|| {
        miette!("git index has no checksum — cannot derive cache-revision signal")
    })?;
    Ok(checksum.to_hex().to_string())
}

// ─── GitReader ─────────────────────────────────────────────────────────────────

/// Opens a gix repository once and reuses it for multiple operations.
///
/// Used by the check codepath under `--source=index|head` to avoid
/// opening the repository on every blob read and path-list query.
pub struct GitReader {
    repo: gix::Repository,
}

#[allow(dead_code)]
impl GitReader {
    /// Open the git repository at `repo_root`.
    pub fn open(repo_root: &Path) -> Result<Self> {
        let repo = open_repo(repo_root)?;
        Ok(GitReader { repo })
    }

    /// Read the blob content for `path_rel` from the given `source`.
    ///
    /// For `Index` this reads from the git index; for `Head` from the HEAD tree.
    /// Returns `Err` on infrastructure failures, `Ok(None)` when the path is absent,
    /// and `Ok(Some(content))` on success.
    pub fn read_blob(&self, source: DocSource, path_rel: &str) -> Result<Option<String>> {
        match source {
            DocSource::Index => read_index_blob_inner(&self.repo, path_rel, 0),
            DocSource::Head => read_head_blob_inner(&self.repo, path_rel, 0),
            DocSource::WorkingTree => {
                unreachable!("GitReader is not used with WorkingTree")
            }
        }
    }

    /// Return `true` when `path_rel` exists in the given `source`.
    pub fn has_entry(&self, source: DocSource, path_rel: &str) -> Result<bool> {
        match source {
            DocSource::Index => {
                let index = open_persisted_index(&self.repo)?;
                Ok(index
                    .entry_by_path(gix::bstr::BStr::new(path_rel.as_bytes()))
                    .is_some())
            }
            DocSource::Head => {
                Ok(read_head_blob_inner(&self.repo, path_rel, 0)?.is_some())
            }
            DocSource::WorkingTree => {
                unreachable!("GitReader is not used with WorkingTree")
            }
        }
    }

    /// Read the raw bytes of the blob at `commit_sha_hex`:`path_rel` (a full
    /// 40-hex commit SHA) — the in-process equivalent of `git show
    /// <sha>:<path>` — reusing this reader's open repository so a multi-
    /// commit history walk pays the open cost once. See
    /// [`read_blob_at_commit`] for the absent/error contract.
    pub fn read_blob_at_commit(
        &self,
        commit_sha_hex: &str,
        path_rel: &str,
    ) -> Result<Option<Vec<u8>>> {
        read_blob_at_commit_inner(&self.repo, commit_sha_hex, path_rel)
    }

    /// Return all repo-relative paths tracked in the given `source`.
    pub fn list_paths(&self, source: DocSource) -> Result<Vec<String>> {
        match source {
            DocSource::Index => {
                let index = open_persisted_index(&self.repo)?;
                let mut paths = Vec::new();
                for entry in index.entries() {
                    paths.push(utf8_repo_path(
                        entry.path(&index),
                        "git index path is not valid UTF-8",
                    )?);
                }
                paths.sort();
                paths.dedup();
                Ok(paths)
            }
            DocSource::Head => {
                let root_tree_id = head_root_tree_id(&self.repo)?;
                let mut paths = Vec::new();
                for_each_tree_entry_recursive(
                    &self.repo,
                    root_tree_id,
                    &mut String::new(),
                    &mut paths,
                )?;
                paths.sort();
                paths.dedup();
                Ok(paths)
            }
            DocSource::WorkingTree => {
                unreachable!("GitReader is not used with WorkingTree")
            }
        }
    }
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
        assert!(
            head_sha(repo.path()).is_err(),
            "expected unborn HEAD to fail"
        );
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

    // ── index / HEAD helper tests (Phase 2 acceptance tests — unskipped in Phase 3) ──

    #[test]
    fn index_tracked_paths_returns_staged_entries() {
        let repo = TestRepo::new();
        repo.create_file("staged.md", "content\n");
        repo.git(&["add", "staged.md"]);

        let paths = index_tracked_paths(repo.path()).expect("index_tracked_paths");
        assert!(paths.contains(&"staged.md".to_owned()));
    }

    #[test]
    fn index_tracked_paths_excludes_untracked() {
        let repo = TestRepo::new();
        repo.create_file("staged.md", "content\n");
        repo.git(&["add", "staged.md"]);
        repo.create_file("untracked.md", "content\n");

        let paths = index_tracked_paths(repo.path()).expect("index_tracked_paths");
        assert!(!paths.contains(&"untracked.md".to_owned()));
    }

    #[test]
    fn read_index_blob_returns_staged_content() {
        // Commit v1, then stage v2; the index holds v2, worktree has v2.
        // The test verifies that index_blob returns the staged (v2) content,
        // not the committed content.
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1\n");
        repo.commit("initial");
        repo.create_file("doc.md", "v2\n");
        repo.git(&["add", "doc.md"]);

        let content = read_index_blob(repo.path(), "doc.md")
            .expect("read_index_blob")
            .expect("expected Some");
        assert_eq!(content, "v2\n");
    }

    #[test]
    fn read_index_blob_returns_none_for_absent_path() {
        let repo = TestRepo::new();
        repo.create_file("other.md", "content\n");
        repo.git(&["add", "other.md"]);

        let result = read_index_blob(repo.path(), "missing.md").expect("read_index_blob");
        assert!(result.is_none());
    }

    #[test]
    fn head_tracked_paths_returns_committed_entries() {
        let repo = TestRepo::new();
        repo.create_file("committed.md", "content\n");
        repo.commit("initial");

        let paths = head_tracked_paths(repo.path()).expect("head_tracked_paths");
        assert!(paths.contains(&"committed.md".to_owned()));
    }

    #[test]
    fn head_tracked_paths_excludes_staged_only() {
        let repo = TestRepo::new();
        repo.create_file("committed.md", "content\n");
        repo.commit("initial");
        repo.create_file("staged_only.md", "content\n");
        repo.git(&["add", "staged_only.md"]);

        let paths = head_tracked_paths(repo.path()).expect("head_tracked_paths");
        assert!(!paths.contains(&"staged_only.md".to_owned()));
    }

    #[test]
    fn read_head_blob_returns_committed_content() {
        // Commit v1, then stage v2; HEAD still holds v1.
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1\n");
        repo.commit("initial");
        repo.create_file("doc.md", "v2\n");
        repo.git(&["add", "doc.md"]);

        let content = read_head_blob(repo.path(), "doc.md")
            .expect("read_head_blob")
            .expect("expected Some");
        assert_eq!(content, "v1\n");
    }

    #[test]
    fn read_head_blob_returns_none_for_absent_path() {
        let repo = TestRepo::new();
        repo.create_file("other.md", "content\n");
        repo.commit("initial");

        let result = read_head_blob(repo.path(), "missing.md").expect("read_head_blob");
        assert!(result.is_none());
    }

    /// Reproduction: when a path resolves through a blob entry (e.g.,
    /// `a.md/sub` where `a.md` is a blob, not a tree), the walk loop sets
    /// `current_tree_id` to the blob's id and the next iteration's
    /// `try_into_tree` fails with an error.  It must return `Ok(None)`
    /// instead — the same graceful "not found" behavior the index-source
    /// twin gives for unresolvable paths.
    ///
    /// MUST FAIL against the current unfixed code.
    #[test]
    fn read_head_blob_returns_none_for_path_through_blob() {
        let repo = TestRepo::new();
        repo.create_file("a.md", "content\n");
        repo.commit("initial");

        let result = read_head_blob(repo.path(), "a.md/sub")
            .expect("read_head_blob must not error for blob-through-tree path");
        assert!(
            result.is_none(),
            "path through a blob must return None, got: {result:?}"
        );
    }

    #[test]
    fn has_index_entry_true_for_staged() {
        let repo = TestRepo::new();
        repo.create_file("staged.md", "content\n");
        repo.git(&["add", "staged.md"]);

        assert!(has_index_entry(repo.path(), "staged.md").expect("has_index_entry"));
    }

    #[test]
    fn has_index_entry_false_for_untracked() {
        let repo = TestRepo::new();
        // Stage a placeholder so `.git/index` exists; the probed path itself
        // is untracked, so the entry lookup must report `false`.
        repo.create_file("placeholder.md", "x\n");
        repo.git(&["add", "placeholder.md"]);
        repo.create_file("untracked.md", "content\n");

        assert!(!has_index_entry(repo.path(), "untracked.md").expect("has_index_entry"));
    }

    /// Finding 1 regression: when `.git/index` is absent, `--source=index`
    /// helpers must error rather than silently substitute HEAD content.
    #[test]
    fn index_helpers_fail_closed_when_index_absent() {
        let repo = TestRepo::new();
        repo.create_file("doc.md", "v1\n");
        repo.commit("initial");
        // Remove the on-disk index — gix would otherwise fall back to a
        // HEAD-derived in-memory index.
        std::fs::remove_file(repo.path().join(".git/index")).expect("remove .git/index");

        assert!(
            index_tracked_paths(repo.path()).is_err(),
            "index_tracked_paths must fail when .git/index is absent"
        );
        assert!(
            read_index_blob(repo.path(), "doc.md").is_err(),
            "read_index_blob must fail when .git/index is absent"
        );
        assert!(
            has_index_entry(repo.path(), "doc.md").is_err(),
            "has_index_entry must fail when .git/index is absent"
        );
        assert!(
            index_revision_signal(repo.path()).is_err(),
            "index_revision_signal must fail when .git/index is absent"
        );
    }

    /// Finding 2 regression: `index_revision_signal` keys on the index file's
    /// content checksum.  Two restages that produce same-size content but
    /// differ in bytes must yield different signals.
    #[test]
    fn index_revision_signal_differs_for_same_size_restage() {
        let repo = TestRepo::new();
        // Same-length payloads so `(mtime, size)` may collide on coarse FS.
        repo.create_file("doc.md", "AAAA\n");
        repo.git(&["add", "doc.md"]);
        let sig_a = index_revision_signal(repo.path()).expect("sig a");

        repo.create_file("doc.md", "BBBB\n");
        repo.git(&["add", "doc.md"]);
        let sig_b = index_revision_signal(repo.path()).expect("sig b");

        assert_ne!(
            sig_a, sig_b,
            "checksum-based index signal must change when staged content changes"
        );
    }

    #[test]
    fn ignored_paths_reports_only_gitignored_candidates() {
        let repo = TestRepo::new();
        repo.create_file(".gitignore", "generated.ts\n");
        repo.create_file("generated.ts", "artifact\n");
        repo.create_file("src/real.ts", "real\n");
        // Untracked-but-not-ignored: present on disk, no .gitignore match.
        repo.create_file("src/new.ts", "new\n");
        repo.commit("init");

        let candidates = vec![
            "generated.ts".to_owned(),
            "src/real.ts".to_owned(),
            "src/new.ts".to_owned(),
        ];
        let ignored = ignored_paths(repo.path(), &candidates).expect("ignored_paths");

        assert!(
            ignored.contains("generated.ts"),
            "gitignored path must be reported"
        );
        assert!(
            !ignored.contains("src/real.ts"),
            "tracked path must not be reported as ignored"
        );
        assert!(
            !ignored.contains("src/new.ts"),
            "untracked-but-not-ignored path must not be reported as ignored"
        );
    }

    #[test]
    fn ignored_paths_empty_when_none_ignored() {
        let repo = TestRepo::new();
        repo.create_file("a.ts", "a\n");
        repo.commit("init");

        // No .gitignore at all → check-ignore exits 1, which must map to an
        // empty set rather than an error.
        let ignored =
            ignored_paths(repo.path(), &["a.ts".to_owned()]).expect("ignored_paths must not error");
        assert!(ignored.is_empty());
    }

    #[test]
    fn ignored_paths_empty_input_returns_empty() {
        let repo = TestRepo::new();
        repo.create_file("a.ts", "a\n");
        repo.commit("init");
        assert!(
            ignored_paths(repo.path(), &[])
                .expect("ignored_paths")
                .is_empty()
        );
    }

    #[test]
    fn has_head_entry_true_for_committed() {
        let repo = TestRepo::new();
        repo.create_file("committed.md", "content\n");
        repo.commit("initial");

        assert!(has_head_entry(repo.path(), "committed.md").expect("has_head_entry"));
    }

    #[cfg(unix)]
    #[test]
    fn read_index_blob_follows_symlinks() {
        let repo = TestRepo::new();
        repo.create_file("real.md", "real content\n");
        // Create a symlink `link.md -> real.md` and stage both.
        std::os::unix::fs::symlink("real.md", repo.path().join("link.md")).expect("create symlink");
        repo.git(&["add", "real.md", "link.md"]);

        let content = read_index_blob(repo.path(), "link.md")
            .expect("read_index_blob")
            .expect("expected Some");
        assert_eq!(content, "real content\n");
    }

    #[cfg(unix)]
    #[test]
    fn read_head_blob_follows_symlinks() {
        let repo = TestRepo::new();
        repo.create_file("real.md", "real content\n");
        std::os::unix::fs::symlink("real.md", repo.path().join("link.md")).expect("create symlink");
        repo.commit("with symlink");

        let content = read_head_blob(repo.path(), "link.md")
            .expect("read_head_blob")
            .expect("expected Some");
        assert_eq!(content, "real content\n");
    }

    #[cfg(unix)]
    #[test]
    fn head_tracked_paths_includes_symlink() {
        let repo = TestRepo::new();
        repo.create_file("real.md", "real\n");
        std::os::unix::fs::symlink("real.md", repo.path().join("link.md")).expect("create symlink");
        repo.commit("with symlink");

        let paths = head_tracked_paths(repo.path()).expect("head_tracked_paths");
        assert!(paths.contains(&"link.md".to_owned()));
        assert!(paths.contains(&"real.md".to_owned()));
    }

    #[test]
    fn has_head_entry_false_for_staged_only() {
        let repo = TestRepo::new();
        repo.create_file("committed.md", "content\n");
        repo.commit("initial");
        repo.create_file("staged_only.md", "content\n");
        repo.git(&["add", "staged_only.md"]);

        assert!(!has_head_entry(repo.path(), "staged_only.md").expect("has_head_entry"));
    }

    // ── read_blob_at_commit (P3 in-process blob reads) ────────────────────────

    /// Multi-commit fixture: create → edit → rename, plus an unrelated file
    /// so each commit's tree is non-trivial. Returns the repo and the SHAs of
    /// the three commits, oldest first.
    fn repo_with_rename_history() -> (TestRepo, [String; 3]) {
        let repo = TestRepo::new();
        repo.create_file("docs/page.md", "v1\n");
        repo.create_file("other.txt", "unrelated\n");
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-m", "create"]);
        let c1 = resolve_ref(repo.path(), "HEAD").expect("HEAD sha");

        repo.create_file("docs/page.md", "v2\n");
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-m", "edit"]);
        let c2 = resolve_ref(repo.path(), "HEAD").expect("HEAD sha");

        repo.git(&["mv", "docs/page.md", "docs/renamed.md"]);
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-m", "rename page"]);
        let c3 = resolve_ref(repo.path(), "HEAD").expect("HEAD sha");

        (repo, [c1, c2, c3])
    }

    /// `git show <sha>:<path>` as the subprocess oracle: `Ok(Some(bytes))`
    /// on success, `Ok(None)` on git's exit-128 absence family.
    fn git_show_blob(repo: &TestRepo, sha: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["show", &format!("{sha}:{path}")])
            .output()
            .into_diagnostic()
            .wrap_err("failed to spawn git show")?;
        if output.status.success() {
            return Ok(Some(output.stdout));
        }
        if output.status.code() == Some(128) {
            return Ok(None);
        }
        Err(miette!(
            "git show {sha}:{path} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    /// The helper must agree with the `git show` subprocess byte-for-byte for
    /// every present path across a create → edit → rename history — including
    /// a directory component — and report absent (`None`) exactly where the
    /// subprocess does: before creation and after the rename.
    #[test]
    fn read_blob_at_commit_matches_git_show_across_history_and_rename() {
        let (repo, [c1, c2, c3]) = repo_with_rename_history();

        // (commit, path, expectation) triples covering present, edited,
        // renamed-away (absent), renamed-to, and never-existed paths.
        let cases = [
            (&c1, "docs/page.md", Some(&b"v1\n"[..])),
            (&c1, "other.txt", Some(&b"unrelated\n"[..])),
            (&c2, "docs/page.md", Some(&b"v2\n"[..])),
            (&c2, "docs/renamed.md", None),
            (&c3, "docs/renamed.md", Some(&b"v2\n"[..])),
            (&c3, "docs/page.md", None),
            (&c1, "docs/nope.md", None),
        ];
        for (sha, path, expected) in cases {
            let got = read_blob_at_commit(repo.path(), sha, path)
                .unwrap_or_else(|e| panic!("read_blob_at_commit {sha}:{path} failed: {e:?}"));
            let want = git_show_blob(&repo, sha, path).expect("git show oracle");
            assert_eq!(got.as_deref(), expected, "fixture sanity at {sha}:{path}");
            assert_eq!(
                got, want,
                "helper must equal `git show {sha}:{path}` (None included)"
            );
        }
    }

    /// The GitReader form must behave identically to the standalone form —
    /// the drift walk holds one open repository across many commit reads.
    #[test]
    fn git_reader_read_blob_at_commit_matches_standalone() {
        let (repo, shas) = repo_with_rename_history();
        let reader = GitReader::open(repo.path()).expect("open reader");
        for sha in shas {
            for path in ["docs/page.md", "docs/renamed.md", "other.txt"] {
                assert_eq!(
                    reader.read_blob_at_commit(&sha, path).expect("reader read"),
                    read_blob_at_commit(repo.path(), &sha, path).expect("standalone read"),
                    "reader and standalone forms disagree at {sha}:{path}"
                );
            }
        }
    }

    /// A non-hex or abbreviated object id is an error, never a silent
    /// `None` — fail closed like every other object failure.
    #[test]
    fn read_blob_at_commit_fails_closed_on_bad_sha() {
        let (repo, _) = repo_with_rename_history();
        for bad in ["not-a-sha", "abcdef"] {
            assert!(
                read_blob_at_commit(repo.path(), bad, "other.txt").is_err(),
                "'{bad}' must error"
            );
        }
        // A well-formed hex id that names no object errors too.
        assert!(read_blob_at_commit(
            repo.path(),
            "0000000000000000000000000000000000000000",
            "other.txt"
        )
        .is_err());
    }
}
