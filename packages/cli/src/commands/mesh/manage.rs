//! In-process `wiki mesh` subcommand: show, add, remove.
//!
//! Provides operator-facing mesh reconciliation so that any `wiki check`
//! failure is resolvable without the `git mesh` binary. All three verbs
//! operate on the `.wiki/` store via [`super::store`].

use std::path::Path;

use miette::Result;

use git_mesh_core::{AnchorExtent, validate_mesh_name, validate_repo_relative_path};

use crate::MeshCommands;

use super::store::{self, UpsertOutcome};

/// Dispatch a `wiki mesh` subcommand.
///
/// Called from `main::run` after `repo_root` is resolved. Returns an exit code
/// compatible with the rest of the CLI (0 = success, 1 = logical failure such
/// as "anchor not found", 2 = usage/IO error).
pub(crate) fn run(command: MeshCommands, repo_root: &Path) -> Result<i32> {
    match command {
        MeshCommands::Show { slug, patch } => show(&slug, patch, repo_root),
        MeshCommands::Add { slug, anchors, why } => add(&slug, &anchors, why.as_deref(), repo_root),
        MeshCommands::Remove { slug, anchor } => remove(&slug, anchor.as_deref(), repo_root),
    }
}

// ── anchor string parser ──────────────────────────────────────────────────────

/// Parse an anchor string of the form `path#Lstart-Lend` or bare `path`.
///
/// Returns `(path, AnchorExtent)`. The path is validated via
/// [`validate_repo_relative_path`]. A `#L…` suffix is required to be
/// well-formed (`#L<start>-L<end>` with 1-based positive integers and
/// `start <= end`); any other suffix is an error.
pub(crate) fn parse_anchor(s: &str) -> Result<(String, AnchorExtent)> {
    let (path_part, extent) = if let Some(hash_pos) = s.find('#') {
        let path_part = &s[..hash_pos];
        let suffix = &s[hash_pos + 1..];
        let extent = parse_line_range_suffix(suffix, s)?;
        (path_part, extent)
    } else {
        (s, AnchorExtent::WholeFile)
    };

    validate_repo_relative_path("anchor path", path_part)
        .map_err(|e| miette::miette!("invalid anchor path `{path_part}`: {e}"))?;

    Ok((path_part.to_string(), extent))
}

/// Parse a `L<start>-L<end>` suffix (the part after `#`).
fn parse_line_range_suffix(suffix: &str, original: &str) -> Result<AnchorExtent> {
    let err = || {
        miette::miette!(
            "malformed anchor `{original}`: expected `path#L<start>-L<end>` \
             (e.g. `src/foo.rs#L2-L4`)"
        )
    };

    let rest = suffix.strip_prefix('L').ok_or_else(err)?;
    let (start_str, end_part) = rest.split_once('-').ok_or_else(err)?;
    let end_str = end_part.strip_prefix('L').ok_or_else(err)?;

    let start: u32 = start_str.parse().map_err(|_| err())?;
    let end: u32 = end_str.parse().map_err(|_| err())?;

    if start == 0 || end == 0 || start > end {
        return Err(miette::miette!(
            "malformed anchor `{original}`: line numbers must be 1-based and start <= end"
        ));
    }

    Ok(AnchorExtent::LineRange { start, end })
}

// ── show ──────────────────────────────────────────────────────────────────────

fn show(slug: &str, patch: bool, repo_root: &Path) -> Result<i32> {
    validate_mesh_name(slug).map_err(|e| miette::miette!("invalid mesh slug `{slug}`: {e}"))?;

    let Some(mesh) = store::read_one(repo_root, slug)? else {
        eprintln!("error: mesh `{slug}` not found");
        return Ok(1);
    };

    println!("mesh: {slug}");
    println!("why:  {}", mesh.why);
    println!();

    for anchor in &mesh.anchors {
        let extent = if anchor.start_line == 0 && anchor.end_line == 0 {
            AnchorExtent::WholeFile
        } else {
            AnchorExtent::LineRange {
                start: anchor.start_line,
                end: anchor.end_line,
            }
        };

        let range_label = if anchor.start_line == 0 && anchor.end_line == 0 {
            "(whole file)".to_string()
        } else {
            format!("L{}-L{}", anchor.start_line, anchor.end_line)
        };

        // Recompute the current hash to detect freshness.
        let current_hash = store::hash_anchor(repo_root, &anchor.path, extent);
        let freshness = match &current_hash {
            Ok(h) if h == &anchor.content_hash => "fresh",
            Ok(_) => "STALE",
            Err(_) => "MISSING",
        };

        println!(
            "  {} {}  stored={}  {}",
            anchor.path,
            range_label,
            &anchor.content_hash[..8.min(anchor.content_hash.len())],
            freshness
        );

        if patch && freshness == "STALE" {
            // Emit a before/after diff for stale anchors.
            let before =
                read_committed_slice(repo_root, &anchor.path, anchor.start_line, anchor.end_line)?;
            let after =
                read_worktree_slice(repo_root, &anchor.path, anchor.start_line, anchor.end_line)?;
            print_diff(
                &anchor.path,
                anchor.start_line,
                anchor.end_line,
                before.as_deref(),
                after.as_deref(),
            );
        }
    }

    Ok(0)
}

/// Read the committed blob slice for a path+range via `git show HEAD:<path>`.
///
/// Returns `None` when the path is not in HEAD (new file).
fn read_committed_slice(
    repo_root: &Path,
    path: &str,
    start: u32,
    end: u32,
) -> Result<Option<String>> {
    use crate::commands::check_fix::read_blob_at;
    let Some(blob) = read_blob_at(repo_root, "HEAD", path)? else {
        return Ok(None);
    };
    Ok(Some(slice_lines(&blob, start, end)))
}

/// Read the worktree file slice for a path+range.
fn read_worktree_slice(
    repo_root: &Path,
    path: &str,
    start: u32,
    end: u32,
) -> Result<Option<String>> {
    let abs = repo_root.join(path);
    match std::fs::read_to_string(&abs) {
        Ok(content) => Ok(Some(slice_lines(&content, start, end))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(miette::miette!("failed to read {}: {e}", abs.display())),
    }
}

/// Extract lines `start..=end` (1-based) from `content`. Whole-file (0/0) returns all.
fn slice_lines(content: &str, start: u32, end: u32) -> String {
    if start == 0 && end == 0 {
        return content.to_string();
    }
    content
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let lineno = (i + 1) as u32;
            if lineno >= start && lineno <= end {
                Some(format!("{line}\n"))
            } else {
                None
            }
        })
        .collect()
}

/// Print a simple before/after diff to stdout.
fn print_diff(path: &str, start: u32, end: u32, before: Option<&str>, after: Option<&str>) {
    let range = if start == 0 && end == 0 {
        "(whole file)".to_string()
    } else {
        format!("L{start}-L{end}")
    };
    println!("  --- a/{path} {range} (committed)");
    println!("  +++ b/{path} {range} (worktree)");
    match (before, after) {
        (None, None) => println!("  (neither committed nor worktree copy found)"),
        (None, Some(a)) => {
            for line in a.lines() {
                println!("  +{line}");
            }
        }
        (Some(b), None) => {
            for line in b.lines() {
                println!("  -{line}");
            }
        }
        (Some(b), Some(a)) => {
            for line in b.lines() {
                println!("  -{line}");
            }
            for line in a.lines() {
                println!("  +{line}");
            }
        }
    }
}

// ── add ───────────────────────────────────────────────────────────────────────

fn add(slug: &str, anchors: &[String], why: Option<&str>, repo_root: &Path) -> Result<i32> {
    validate_mesh_name(slug).map_err(|e| miette::miette!("invalid mesh slug `{slug}`: {e}"))?;

    for anchor_str in anchors {
        let (path, extent) = parse_anchor(anchor_str)?;

        let outcome = store::upsert_anchor(repo_root, slug, why, &path, extent)?;

        let anchor_label = extent_label(&path, extent);
        match outcome {
            UpsertOutcome::Created => println!("created mesh `{slug}` with anchor {anchor_label}"),
            UpsertOutcome::Extended => {
                println!("extended mesh `{slug}` with anchor {anchor_label}")
            }
            UpsertOutcome::Refreshed => {
                println!("refreshed anchor {anchor_label} in mesh `{slug}`")
            }
        }

        // After first anchor creates the mesh, subsequent anchors in the same
        // invocation no longer need --why (the mesh exists).
    }

    Ok(0)
}

/// Format an anchor extent as a human-readable label.
fn extent_label(path: &str, extent: AnchorExtent) -> String {
    match extent {
        AnchorExtent::WholeFile => path.to_string(),
        AnchorExtent::LineRange { start, end } => format!("{path}#L{start}-L{end}"),
    }
}

// ── remove ────────────────────────────────────────────────────────────────────

fn remove(slug: &str, anchor: Option<&str>, repo_root: &Path) -> Result<i32> {
    validate_mesh_name(slug).map_err(|e| miette::miette!("invalid mesh slug `{slug}`: {e}"))?;

    if let Some(anchor_str) = anchor {
        // Remove a single anchor.
        let (path, extent) = parse_anchor(anchor_str)?;
        let (start, end) = match extent {
            AnchorExtent::WholeFile => (0u32, 0u32),
            AnchorExtent::LineRange { start, end } => (start, end),
        };

        let removed = store::remove_anchor(repo_root, slug, &path, start, end)?;
        if removed {
            println!(
                "removed anchor {} from mesh `{slug}`",
                extent_label(&path, extent)
            );
        } else {
            eprintln!(
                "error: anchor {} not found in mesh `{slug}`",
                extent_label(&path, extent)
            );
            return Ok(1);
        }
    } else {
        // Remove the whole mesh.
        if !store::exists(repo_root, slug) {
            eprintln!("error: mesh `{slug}` not found");
            return Ok(1);
        }
        store::delete(repo_root, slug)?;
        println!("deleted mesh `{slug}`");
    }

    Ok(0)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── anchor parser tests ───────────────────────────────────────────────────

    #[test]
    fn parse_anchor_line_range() {
        let (path, extent) = parse_anchor("src/foo.rs#L2-L4").unwrap();
        assert_eq!(path, "src/foo.rs");
        assert_eq!(extent, AnchorExtent::LineRange { start: 2, end: 4 });
    }

    #[test]
    fn parse_anchor_whole_file() {
        let (path, extent) = parse_anchor("src/bar.rs").unwrap();
        assert_eq!(path, "src/bar.rs");
        assert_eq!(extent, AnchorExtent::WholeFile);
    }

    #[test]
    fn parse_anchor_malformed_suffix_is_error() {
        let result = parse_anchor("src/foo.rs#2-4");
        assert!(result.is_err(), "missing L prefix must be an error");
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("malformed"), "error must say malformed: {msg}");
    }

    #[test]
    fn parse_anchor_malformed_no_end_is_error() {
        let result = parse_anchor("src/foo.rs#L2");
        assert!(result.is_err(), "missing end line must be an error");
    }

    #[test]
    fn parse_anchor_zero_start_is_error() {
        let result = parse_anchor("src/foo.rs#L0-L4");
        assert!(result.is_err(), "line 0 must be an error");
    }

    #[test]
    fn parse_anchor_start_greater_than_end_is_error() {
        let result = parse_anchor("src/foo.rs#L5-L2");
        assert!(result.is_err(), "start > end must be an error");
    }
}
