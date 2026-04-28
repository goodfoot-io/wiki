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
pub(super) fn collect_mesh_diagnostics(
    files: &[PathBuf],
    repo_root: &Path,
) -> Vec<CheckDiagnostic> {
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

            let rows = cache
                .entry((target.clone(), start, end))
                .or_insert_with(|| {
                    run_git_mesh_ls(repo_root, &target, start, end, &mut probed, &mut out)
                });

            // probed may have become Missing during the or_insert_with above
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

    out
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
/// diagnostic, set `probed = Missing`, return empty vec.
///
/// On non-zero exit: push a runtime diagnostic (treated as exit-2 by caller)
/// and return empty vec.
fn run_git_mesh_ls(
    repo_root: &Path,
    target: &Path,
    start: u32,
    end: u32,
    probed: &mut MeshProbe,
    out: &mut Vec<CheckDiagnostic>,
) -> Vec<MeshRow> {
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
            Vec::new()
        }
        Err(e) => {
            // Unexpected OS error — treat as runtime failure but don't abort
            out.push(CheckDiagnostic {
                kind: "runtime".into(),
                file: String::new(),
                line: 0,
                message: format!("git mesh ls failed: {e}"),
            });
            Vec::new()
        }
        Ok(output) => {
            *probed = MeshProbe::Available;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                out.push(CheckDiagnostic {
                    kind: "runtime".into(),
                    file: String::new(),
                    line: 0,
                    message: format!(
                        "git mesh ls exited with status {}: {}",
                        output.status,
                        stderr.trim()
                    ),
                });
                return Vec::new();
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_mesh_ls_output(&stdout)
        }
    }
}

/// Parse the `--porcelain` output of `git mesh ls`.
///
/// Each non-empty line: `<mesh-name>\t<path>\t<start>-<end>`
/// The sentinel `no meshes` (single line, no tabs) maps to an empty vec.
fn parse_mesh_ls_output(stdout: &str) -> Vec<MeshRow> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line == "no meshes" {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() != 3 {
            continue;
        }
        let mesh = parts[0].to_string();
        let path = PathBuf::from(parts[1]);
        let _range = parts[2]; // git mesh ls already applies range filtering server-side
        rows.push(MeshRow { mesh, path });
    }
    rows
}

