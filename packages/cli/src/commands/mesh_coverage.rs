use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::commands::resolve_link_path;
use crate::parser::{LinkKind, parse_fragment_links};

use super::check::CheckDiagnostic;

// ── Types ─────────────────────────────────────────────────────────────────────

/// All mesh data fetched in a single `git-mesh list --porcelain` call.
pub(crate) struct MeshIndex {
    /// Code anchor `(path, start, end)` → names of every mesh containing it.
    by_anchor: HashMap<(PathBuf, u32, u32), Vec<String>>,
    /// Mesh name → every path anchored by that mesh (any range).
    paths_by_mesh: HashMap<String, Vec<PathBuf>>,
}

impl MeshIndex {
    pub(crate) fn is_covered(&self, code_path: &Path, start: u32, end: u32, wiki_rel: &Path) -> bool {
        // Check exact-range anchor and the whole-file sentinel (0-0).
        let keys: &[(PathBuf, u32, u32)] = &[
            (code_path.to_path_buf(), start, end),
            (code_path.to_path_buf(), 0, 0),
        ];
        keys.iter()
            .filter_map(|k| self.by_anchor.get(k))
            .flatten()
            .any(|name| {
                self.paths_by_mesh
                    .get(name)
                    .is_some_and(|paths| paths.iter().any(|p| paths_equal(p, wiki_rel)))
            })
    }
}

// ── Public surface ────────────────────────────────────────────────────────────

/// Collect `mesh_uncovered` and `mesh_unavailable` diagnostics for the given
/// wiki files.
///
/// Invokes `git-mesh list --porcelain` exactly once to fetch all mesh data, then
/// performs all coverage lookups in memory.
///
/// Returns `Err` if `git-mesh` fails for any reason other than `NotFound`
/// (which is treated as a `mesh_unavailable` diagnostic in the `Ok` vec).
pub(super) fn collect_mesh_diagnostics(
    files: &[PathBuf],
    repo_root: &Path,
) -> Result<Vec<CheckDiagnostic>, miette::Error> {
    let mut out: Vec<CheckDiagnostic> = Vec::new();

    if files.is_empty() {
        return Ok(out);
    }

    let rel_paths: Vec<PathBuf> = files
        .iter()
        .map(|p| {
            p.strip_prefix(repo_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| p.clone())
        })
        .collect();

    let index = match run_git_mesh_ls_all(repo_root, &rel_paths, &mut out)? {
        None => return Ok(out), // git-mesh unavailable; mesh_unavailable diagnostic already pushed
        Some(idx) => idx,
    };

    for wiki_path in files {
        let content = match std::fs::read_to_string(wiki_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let wiki_rel = wiki_path
            .strip_prefix(repo_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| wiki_path.clone());

        for link in parse_fragment_links(&content) {
            if link.kind == LinkKind::External {
                continue;
            }
            let Some(start) = link.start_line else {
                continue;
            };
            let end = link.end_line.unwrap_or(start);
            let target = resolve_link_path(&link.path, wiki_path, repo_root);

            // Skip if target is a directory (consistent with the missing_file check)
            let abs_target = repo_root.join(&target);
            if abs_target.is_dir() {
                continue;
            }

            if !index.is_covered(&target, start, end, &wiki_rel) {
                out.push(CheckDiagnostic {
                    kind: "mesh_uncovered".into(),
                    file: wiki_path.display().to_string(),
                    line: link.source_line,
                    message: format!(
                        "fragment link `{}#L{start}-L{end}` has no covering mesh",
                        link.path
                    ),
                });
            }
        }
    }

    Ok(out)
}

/// Build a `MeshIndex` for the given wiki files by invoking `git-mesh list
/// --porcelain --batch` once.
///
/// Returns `Ok(None)` when the `git-mesh` binary is not on PATH so callers can
/// decide how to react (e.g. `wiki check` surfaces a `mesh_unavailable`
/// diagnostic; `wiki scaffold` fails closed). Returns `Err` for any other
/// runtime failure.
pub(crate) fn build_mesh_index(
    repo_root: &Path,
    files: &[PathBuf],
) -> Result<Option<MeshIndex>, miette::Error> {
    if files.is_empty() {
        return Ok(Some(MeshIndex {
            by_anchor: HashMap::new(),
            paths_by_mesh: HashMap::new(),
        }));
    }
    let rel_paths: Vec<PathBuf> = files
        .iter()
        .map(|p| {
            p.strip_prefix(repo_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| p.clone())
        })
        .collect();
    let mut sink: Vec<CheckDiagnostic> = Vec::new();
    run_git_mesh_ls_all(repo_root, &rel_paths, &mut sink)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Normalize-compare two paths by components, handling leading `./`.
fn paths_equal(a: &Path, b: &Path) -> bool {
    use std::path::Component;
    let a_components: Vec<_> = a
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    let b_components: Vec<_> = b
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    a_components == b_components
}

/// Shell out to `git-mesh list --porcelain --batch` with repo-relative wiki file
/// paths piped to stdin, filtering output to only meshes that anchor at least
/// one of the given paths.
///
/// Returns `Ok(None)` when git-mesh is not installed, having pushed a
/// `mesh_unavailable` diagnostic into `out`.
///
/// Returns `Err` on any other OS error or non-zero exit.
fn run_git_mesh_ls_all(
    repo_root: &Path,
    files: &[PathBuf],
    out: &mut Vec<CheckDiagnostic>,
) -> Result<Option<MeshIndex>, miette::Error> {
    let mut cmd = Command::new("git-mesh");
    cmd.current_dir(repo_root)
        .args(["list", "--porcelain", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            out.push(CheckDiagnostic {
                kind: "mesh_unavailable".into(),
                file: String::new(),
                line: 0,
                message: "git mesh is required by `wiki check` but was not found on PATH. Install git-mesh and re-run; see https://github.com/goodfoot-io/git-mesh for setup.".into(),
            });
            return Ok(None);
        }
        Err(e) => return Err(miette::miette!("git mesh list failed: {e}")),
        Ok(child) => child,
    };

    if let Some(mut stdin) = child.stdin.take() {
        for path in files {
            writeln!(stdin, "{}", path.display())
                .map_err(|e| miette::miette!("failed to write path to git-mesh stdin: {e}"))?;
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| miette::miette!("git mesh list failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "git mesh list exited with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(Some(parse_mesh_ls_output(&stdout)?))
}

/// Parse the `--porcelain` output of `git mesh list` into a `MeshIndex`.
///
/// Each non-empty line: `<mesh-name>\t<path>\t<start>-<end>`
///
/// The mesh name may contain tabs; the format is parsed right-to-left:
///   1. Peel the range token (`\d+-\d+`) from the right.
///   2. Peel the path from the right of the remainder.
///   3. Whatever is left is the mesh name.
///
/// The sentinel `no meshes` maps to an empty index.
///
/// Returns `Err` if any line cannot be parsed (fatal runtime error).
fn parse_mesh_ls_output(stdout: &str) -> Result<MeshIndex, miette::Error> {
    let mut by_anchor: HashMap<(PathBuf, u32, u32), Vec<String>> = HashMap::new();
    let mut paths_by_mesh: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line == "no meshes" {
            continue;
        }
        // Peel the range token from the right
        let mut right = line.rsplitn(2, '\t');
        let range_token = right.next().unwrap_or("");
        let prefix = match right.next() {
            Some(p) => p,
            None => {
                return Err(miette::miette!(
                    "git mesh list: unparseable output line (no tab-separated range): {line:?}"
                ));
            }
        };
        let (start, end) = parse_range_token(range_token).ok_or_else(|| {
            miette::miette!(
                "git mesh list: unparseable output line (invalid range token {range_token:?}): {line:?}"
            )
        })?;
        // Peel the path from the right of the prefix
        let mut mid = prefix.rsplitn(2, '\t');
        let path_str = mid.next().unwrap_or("");
        let mesh_name = match mid.next() {
            Some(m) => m,
            None => {
                return Err(miette::miette!(
                    "git mesh list: unparseable output line (no mesh name): {line:?}"
                ));
            }
        };
        let path = PathBuf::from(path_str);
        let mesh = mesh_name.to_string();

        by_anchor
            .entry((path.clone(), start, end))
            .or_default()
            .push(mesh.clone());

        let mesh_paths = paths_by_mesh.entry(mesh).or_default();
        if !mesh_paths.iter().any(|p| paths_equal(p, &path)) {
            mesh_paths.push(path);
        }
    }

    Ok(MeshIndex { by_anchor, paths_by_mesh })
}

/// Parse a `\d+-\d+` range token into `(start, end)`. Returns `None` on failure.
fn parse_range_token(token: &str) -> Option<(u32, u32)> {
    let (a, b) = token.split_once('-')?;
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let start = a.parse::<u32>().ok()?;
    let end = b.parse::<u32>().ok()?;
    Some((start, end))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build(output: &str) -> MeshIndex {
        parse_mesh_ls_output(output).expect("parse")
    }

    #[test]
    fn parse_simple_row() {
        let idx = build("my-mesh\tpkg/file.rs\t1-10\n");
        assert!(idx.is_covered(Path::new("pkg/file.rs"), 1, 10, Path::new("pkg/file.rs")));
    }

    #[test]
    fn parse_no_meshes_sentinel() {
        let idx = build("no meshes\n");
        assert!(idx.by_anchor.is_empty());
        assert!(idx.paths_by_mesh.is_empty());
    }

    #[test]
    fn parse_tab_bearing_mesh_slug() {
        let idx = build("foo\tbar\tpkg/file.rs\t1-1\n");
        assert!(idx.paths_by_mesh.contains_key("foo\tbar"));
    }

    #[test]
    fn parse_unparseable_line_returns_err() {
        assert!(parse_mesh_ls_output("this-has-no-tabs-at-all\n").is_err());
    }

    #[test]
    fn parse_invalid_range_token_returns_err() {
        assert!(parse_mesh_ls_output("my-mesh\tpkg/file.rs\tnot-a-range\n").is_err());
    }

    #[test]
    fn coverage_requires_both_anchors_in_same_mesh() {
        // mesh-a has code + wiki; mesh-b has code only → covered
        let idx = build(
            "mesh-a\tsrc/code.rs\t1-10\n\
             mesh-a\twiki/page.md\t1-1\n\
             mesh-b\tsrc/code.rs\t1-10\n",
        );
        assert!(idx.is_covered(Path::new("src/code.rs"), 1, 10, Path::new("wiki/page.md")));
    }

    #[test]
    fn coverage_fails_when_no_mesh_has_wiki_file() {
        // mesh has code but not the wiki file
        let idx = build("mesh-a\tsrc/code.rs\t1-10\n");
        assert!(!idx.is_covered(Path::new("src/code.rs"), 1, 10, Path::new("wiki/page.md")));
    }

    #[test]
    fn coverage_fails_for_wrong_range() {
        let idx = build(
            "mesh-a\tsrc/code.rs\t5-15\n\
             mesh-a\twiki/page.md\t1-1\n",
        );
        // querying range 1-10 doesn't match the stored 5-15 (and it's not a whole-file 0-0)
        assert!(!idx.is_covered(Path::new("src/code.rs"), 1, 10, Path::new("wiki/page.md")));
    }

    #[test]
    fn whole_file_anchor_covers_any_range() {
        // Whole-file anchors are stored as 0-0 by git-mesh
        let idx = build(
            "mesh-a\tsrc/code.rs\t0-0\n\
             mesh-a\twiki/page.md\t1-1\n",
        );
        assert!(idx.is_covered(Path::new("src/code.rs"), 1, 1, Path::new("wiki/page.md")));
        assert!(idx.is_covered(Path::new("src/code.rs"), 10, 20, Path::new("wiki/page.md")));
    }

    #[test]
    fn paths_equal_ignores_leading_dotslash() {
        assert!(paths_equal(Path::new("./foo/bar"), Path::new("foo/bar")));
    }
}
