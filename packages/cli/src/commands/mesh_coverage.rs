use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::commands::resolve_link_path;
use crate::parser::{LinkKind, parse_fragment_links};

use super::check::CheckDiagnostic;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct MeshRow {
    mesh: String,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MeshProbe {
    Unknown,
    Available,
    Missing,
}

impl MeshProbe {
    fn is_missing(&self) -> bool {
        *self == MeshProbe::Missing
    }
}

// ── Public surface ────────────────────────────────────────────────────────────

/// Collect `mesh_uncovered` and `mesh_unavailable` diagnostics for the given
/// wiki files.
///
/// Invokes `git mesh ls --porcelain` at most once per unique
/// `(target_path, start_line, end_line)` tuple across the entire call.
///
/// Returns `Err` if `git-mesh` fails for any reason other than `NotFound`
/// (which is treated as a `mesh_unavailable` diagnostic in the `Ok` vec).
pub(super) fn collect_mesh_diagnostics(
    files: &[PathBuf],
    repo_root: &Path,
) -> Result<Vec<CheckDiagnostic>, miette::Error> {
    let mut out: Vec<CheckDiagnostic> = Vec::new();
    // Cache key: (repo-relative target path, start, end) → rows returned by git mesh ls
    let mut cache: HashMap<(PathBuf, u32, u32), Vec<MeshRow>> = HashMap::new();
    let mut probed = MeshProbe::Unknown;

    for wiki_path in files {
        let content = match std::fs::read_to_string(wiki_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

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

            // If git mesh is unavailable, the diagnostic has already been pushed; stop.
            if probed.is_missing() {
                break;
            }

            let cache_key = (target.clone(), start, end);
            if !cache.contains_key(&cache_key) {
                let rows = run_git_mesh_ls(repo_root, &target, start, end, &mut probed, &mut out)?;
                cache.insert(cache_key.clone(), rows);
            }
            let rows = &cache[&cache_key];

            // probed may have become Missing during the run above
            if probed.is_missing() {
                break;
            }

            let wiki_rel = wiki_path
                .strip_prefix(repo_root)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| wiki_path.clone());

            let covered = is_covered(rows, &wiki_rel);

            if !covered {
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns true iff some mesh in `rows` also has `wiki_rel` as an anchor.
fn is_covered(rows: &[MeshRow], wiki_rel: &Path) -> bool {
    // Collect all mesh names present in the rows
    let mesh_names: HashSet<&str> = rows.iter().map(|r| r.mesh.as_str()).collect();
    // For each mesh that covers the code anchor, check if the wiki file is also anchored
    mesh_names.iter().any(|mesh_name| {
        rows.iter()
            .any(|r| r.mesh.as_str() == *mesh_name && paths_equal(&r.path, wiki_rel))
    })
}

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

/// Shell out to `git mesh ls <target>#L<start>-L<end> --porcelain`.
///
/// On `ErrorKind::NotFound` (git-mesh not installed): push a `mesh_unavailable`
/// diagnostic, set `probed = Missing`, return `Ok(empty vec)`.
///
/// On any other OS error or non-zero exit: return `Err` so the caller treats it
/// as a runtime/infra failure (exit code 2).
fn run_git_mesh_ls(
    repo_root: &Path,
    target: &Path,
    start: u32,
    end: u32,
    probed: &mut MeshProbe,
    out: &mut Vec<CheckDiagnostic>,
) -> Result<Vec<MeshRow>, miette::Error> {
    let anchor = format!("{}#L{}-L{}", target.display(), start, end);
    let result = Command::new("git-mesh")
        .current_dir(repo_root)
        .args(["ls", &anchor, "--porcelain"])
        .output();

    match result {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            *probed = MeshProbe::Missing;
            out.push(CheckDiagnostic {
                kind: "mesh_unavailable".into(),
                file: String::new(),
                line: 0,
                message: "git mesh is not installed; skipped --mesh coverage check".into(),
            });
            Ok(Vec::new())
        }
        Err(e) => {
            // Unexpected OS error — fatal runtime failure
            Err(miette::miette!("git mesh ls failed: {e}"))
        }
        Ok(output) => {
            *probed = MeshProbe::Available;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(miette::miette!(
                    "git mesh ls exited with status {}: {}",
                    output.status,
                    stderr.trim()
                ));
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_mesh_ls_output(&stdout)
        }
    }
}

/// Parse the `--porcelain` output of `git mesh ls`.
///
/// Each non-empty line: `<mesh-name>\t<path>\t<start>-<end>`
///
/// The mesh name may contain tabs; the format is parsed right-to-left:
///   1. Peel off the trailing range token (`\d+-\d+`) with `rsplitn(2, '\t')`.
///   2. From the remainder, peel off the path with `rsplitn(2, '\t')`.
///   3. Whatever is left is the mesh name (may contain tabs).
///
/// The sentinel `no meshes` (single line, no tabs) maps to an empty vec.
///
/// Returns `Err` if a line cannot be parsed (fatal runtime error).
fn parse_mesh_ls_output(stdout: &str) -> Result<Vec<MeshRow>, miette::Error> {
    let mut rows = Vec::new();
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
                    "git mesh ls: unparseable output line (no tab-separated range): {line:?}"
                ));
            }
        };
        // Validate range token matches \d+-\d+
        if !range_token_is_valid(range_token) {
            return Err(miette::miette!(
                "git mesh ls: unparseable output line (invalid range token {range_token:?}): {line:?}"
            ));
        }
        // Peel the path from the right of the prefix
        let mut mid = prefix.rsplitn(2, '\t');
        let path_str = mid.next().unwrap_or("");
        let mesh_name = match mid.next() {
            Some(m) => m,
            None => {
                return Err(miette::miette!(
                    "git mesh ls: unparseable output line (no mesh name): {line:?}"
                ));
            }
        };
        let mesh = mesh_name.to_string();
        let path = PathBuf::from(path_str);
        let _range = range_token; // range filtering is applied server-side
        rows.push(MeshRow { mesh, path });
    }
    Ok(rows)
}

/// Returns true if `token` matches `\d+-\d+`.
fn range_token_is_valid(token: &str) -> bool {
    if let Some((a, b)) = token.split_once('-') {
        !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_row() {
        let output = "my-mesh\tpkg/file.rs\t1-10\n";
        let rows = parse_mesh_ls_output(output).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mesh, "my-mesh");
        assert_eq!(rows[0].path, PathBuf::from("pkg/file.rs"));
    }

    #[test]
    fn parse_no_meshes_sentinel() {
        let rows = parse_mesh_ls_output("no meshes\n").expect("parse");
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_tab_bearing_mesh_slug() {
        // mesh name contains a tab: "foo\tbar", path is "pkg/file.rs", range is "1-1"
        let output = "foo\tbar\tpkg/file.rs\t1-1\n";
        let rows = parse_mesh_ls_output(output).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mesh, "foo\tbar");
        assert_eq!(rows[0].path, PathBuf::from("pkg/file.rs"));
    }

    #[test]
    fn parse_unparseable_line_returns_err() {
        // A line with no tab-separated range token
        let output = "this-has-no-tabs-at-all\n";
        let result = parse_mesh_ls_output(output);
        assert!(result.is_err(), "expected Err for unparseable line");
    }

    #[test]
    fn parse_invalid_range_token_returns_err() {
        // Looks like three parts but range is not numeric
        let output = "my-mesh\tpkg/file.rs\tnot-a-range\n";
        let result = parse_mesh_ls_output(output);
        assert!(result.is_err(), "expected Err for invalid range token");
    }
}
