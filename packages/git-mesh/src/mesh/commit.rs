use crate::git::{
    apply_ref_transaction, git_stdout, git_stdout_with_identity, git_with_input,
    is_reference_transaction_conflict, resolve_ref_oid_optional, RefUpdate,
};
use crate::link::{
    build_link, normalize_range_specs, normalize_side_specs, read_link_from_ref, write_link_blob,
};
use crate::mesh::read::read_mesh_links;
use crate::types::*;
use crate::validation::validate_mesh_name;
use anyhow::{Result, anyhow};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

const COMMIT_MESH_MAX_ATTEMPTS: usize = 8;
static TEST_HOOKS_RUN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub fn commit_mesh(repo: &gix::Repository, input: CommitInput) -> Result<()> {
    // ----------------------------------------------------------------
    // Phase 1: static validation (no repo reads beyond rev-parse, no writes).
    // Per §6.2, every check that does not depend on the mesh's current tip
    // runs before any `hash-object` / `mktree` / `commit-tree` / `update-ref`.
    // ----------------------------------------------------------------
    validate_mesh_name(&input.name)?;

    if input.amend && (!input.adds.is_empty() || !input.removes.is_empty()) {
        anyhow::bail!(
            "--amend is reword-only and cannot be combined with --link or --unlink"
        );
    }
    if !input.amend && input.adds.is_empty() && input.removes.is_empty() {
        anyhow::bail!(
            "nothing to commit: supply --link, --unlink, or --amend -m <msg>"
        );
    }

    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mesh_ref = format!("refs/meshes/v1/{}", input.name);
    let explicit_expected_tip = match input.expected_tip.as_deref() {
        Some(expected_tip) => Some(git_stdout(work_dir, ["rev-parse", expected_tip])?),
        None => None,
    };

    let anchor_sha = match input.anchor_sha {
        Some(anchor_sha) => anchor_sha,
        None => git_stdout(work_dir, ["rev-parse", "HEAD"])?,
    };

    // Pre-normalize adds and removes once so retry attempts reuse the
    // result and error messages can cite the pre-sort pair unchanged.
    let normalized_adds: Vec<[SideSpec; 2]> = input
        .adds
        .iter()
        .cloned()
        .map(normalize_side_specs)
        .collect();
    let normalized_removes: Vec<[RangeSpec; 2]> = input
        .removes
        .iter()
        .cloned()
        .map(normalize_range_specs)
        .collect();

    let mut base_tip = match explicit_expected_tip.clone() {
        Some(explicit) => Some(explicit),
        None => resolve_ref_oid_optional(work_dir, &mesh_ref)?,
    };

    for attempt in 0..COMMIT_MESH_MAX_ATTEMPTS {
        if base_tip.is_none() && normalized_adds.is_empty() {
            anyhow::bail!(
                "mesh `{}` does not exist; supply --link to create it",
                input.name
            );
        }

        let starting_links = match base_tip.as_deref() {
            Some(tip) => read_mesh_links(repo, &gix::ObjectId::from_hex(tip.as_bytes())?)?,
            None => Vec::new(),
        };

        // ------------------------------------------------------------
        // Phase 2: state-dependent validation against the observed tip.
        // Simulate removes, then validate adds against the post-remove
        // set. No objects are written in this phase.
        // ------------------------------------------------------------
        let mut links = starting_links.clone();
        for sides in &normalized_removes {
            let Some(index) = find_mesh_link_index(work_dir, &links, sides)? else {
                anyhow::bail!(
                    "--unlink: mesh `{}` does not contain link {}",
                    input.name,
                    format_pair(&sides[0], &sides[1])
                );
            };
            links.remove(index);
        }
        for (index, sides) in normalized_adds.iter().enumerate() {
            if mesh_contains_sides(work_dir, &links, sides)? {
                let pair = format_pair_from_sides(&sides[0], &sides[1]);
                anyhow::bail!(
                    "--link: mesh `{}` already contains link {pair}; \
                     use `--unlink {pair} --link {pair}` to re-anchor at HEAD",
                    input.name
                );
            }
            // Intra-invocation duplicate --link detection: two --link flags
            // naming the same pair in one call must error before any write.
            for earlier in &normalized_adds[..index] {
                if side_specs_match(earlier, sides) {
                    let pair = format_pair_from_sides(&sides[0], &sides[1]);
                    anyhow::bail!(
                        "--link: pair {pair} appears more than once in this invocation"
                    );
                }
            }
        }

        // ------------------------------------------------------------
        // Phase 3: build objects. All validation has passed; now write.
        // ------------------------------------------------------------
        let mut links = starting_links.clone();
        for sides in &normalized_removes {
            // Unwrap is safe: phase 2 just proved every remove target exists
            // against this same observed tip.
            let index = find_mesh_link_index(work_dir, &links, sides)?
                .expect("validated in phase 2");
            links.remove(index);
        }

        let mut ref_updates = Vec::with_capacity(normalized_adds.len() + 1);
        for sides in &normalized_adds {
            let link = build_link(work_dir, &anchor_sha, sides.clone())?;
            let id = Uuid::new_v4().to_string();
            let blob_oid = write_link_blob(work_dir, &link)?;
            ref_updates.push(RefUpdate::Create {
                name: format!("refs/links/v1/{id}"),
                new_oid: blob_oid,
            });
            links.push(id);
        }

        let mesh_commit = build_mesh_commit(
            work_dir,
            &input.name,
            &input.message,
            &links,
            base_tip.as_deref(),
            input.amend,
        )?;
        ref_updates.push(match base_tip.as_deref() {
            Some(expected_old_oid) => RefUpdate::Update {
                name: mesh_ref.clone(),
                new_oid: mesh_commit,
                expected_old_oid: expected_old_oid.to_string(),
            },
            None => RefUpdate::Create {
                name: mesh_ref.clone(),
                new_oid: mesh_commit,
            },
        });

        run_test_hook(work_dir, "commit_mesh_before_transaction");

        match apply_ref_transaction(work_dir, &ref_updates) {
            Ok(()) => return Ok(()),
            Err(err)
                if explicit_expected_tip.is_none()
                    && is_reference_transaction_conflict(&err)
                    && attempt + 1 < COMMIT_MESH_MAX_ATTEMPTS =>
            {
                base_tip = resolve_ref_oid_optional(work_dir, &mesh_ref)?;
            }
            Err(err) => return Err(err),
        }
    }

    anyhow::bail!("mesh commit exceeded retry budget")
}

fn format_pair(a: &RangeSpec, b: &RangeSpec) -> String {
    format!(
        "{}#L{}-L{}:{}#L{}-L{}",
        a.path, a.start, a.end, b.path, b.start, b.end
    )
}

fn format_pair_from_sides(a: &SideSpec, b: &SideSpec) -> String {
    format!(
        "{}#L{}-L{}:{}#L{}-L{}",
        a.path, a.start, a.end, b.path, b.start, b.end
    )
}

fn side_specs_match(a: &[SideSpec; 2], b: &[SideSpec; 2]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| {
        x.path == y.path && x.start == y.start && x.end == y.end
    })
}

fn find_mesh_link_index(
    work_dir: &Path,
    links: &[String],
    sides: &[RangeSpec; 2],
) -> Result<Option<usize>> {
    for (index, link_id) in links.iter().enumerate() {
        let link = read_link_from_ref(work_dir, link_id)?;
        if link_matches_ranges(&link, sides) {
            return Ok(Some(index));
        }
    }

    Ok(None)
}

fn mesh_contains_sides(work_dir: &Path, links: &[String], sides: &[SideSpec; 2]) -> Result<bool> {
    for link_id in links {
        let link = read_link_from_ref(work_dir, link_id)?;
        if link_matches_sides(&link, sides) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn link_matches_sides(link: &Link, sides: &[SideSpec; 2]) -> bool {
    link.sides
        .iter()
        .zip(sides.iter())
        .all(|(existing, candidate)| {
            existing.path == candidate.path
                && existing.start == candidate.start
                && existing.end == candidate.end
                && existing.copy_detection
                    == candidate.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION)
                && existing.ignore_whitespace
                    == candidate
                        .ignore_whitespace
                        .unwrap_or(DEFAULT_IGNORE_WHITESPACE)
        })
}

fn link_matches_ranges(link: &Link, sides: &[RangeSpec; 2]) -> bool {
    link.sides
        .iter()
        .zip(sides.iter())
        .all(|(existing, candidate)| {
            existing.path == candidate.path
                && existing.start == candidate.start
                && existing.end == candidate.end
        })
}

fn canonicalize_links(links: &[String]) -> Vec<String> {
    let mut canonical = links.to_vec();
    canonical.sort();
    canonical.dedup();
    canonical
}

fn serialize_links_file(links: &[String]) -> String {
    let mut links_text = String::new();
    for link in canonicalize_links(links) {
        links_text.push_str(&link);
        links_text.push('\n');
    }
    links_text
}

fn build_mesh_commit(
    work_dir: &Path,
    _name: &str,
    message: &str,
    links: &[String],
    expected_tip: Option<&str>,
    amend: bool,
) -> Result<String> {
    let links_text = serialize_links_file(links);

    let links_blob = git_with_input(work_dir, ["hash-object", "-w", "--stdin"], &links_text)?;
    let tree_oid = git_with_input(
        work_dir,
        ["mktree"],
        &format!("100644 blob {links_blob}\tlinks\n"),
    )?;

    let parents = match (amend, expected_tip) {
        (true, Some(tip)) => git_stdout(work_dir, ["show", "-s", "--format=%P", tip])?
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        (true, None) => Vec::new(),
        (false, Some(tip)) => vec![tip.to_string()],
        (false, None) => Vec::new(),
    };

    let mut args = vec![
        "commit-tree".to_string(),
        tree_oid,
        "-m".to_string(),
        message.to_string(),
    ];
    for parent in parents {
        args.push("-p".to_string());
        args.push(parent);
    }

    git_stdout_with_identity(work_dir, args.iter().map(String::as_str))
}

pub(crate) fn run_test_hook(work_dir: &Path, hook_name: &str) {
    let Ok(config) = std::env::var("GIT_MESH_TEST_HOOK") else {
        return;
    };
    let mut parts = config.splitn(3, ':');
    let Some(expected_hook) = parts.next() else {
        return;
    };
    let remaining = parts.next();
    let command = parts.next();
    if expected_hook != hook_name || remaining != Some("once") {
        return;
    }
    let seen = TEST_HOOKS_RUN.get_or_init(|| Mutex::new(HashSet::new()));
    let mut seen = seen.lock().expect("test hook mutex poisoned");
    if !seen.insert(config.clone()) {
        return;
    }
    drop(seen);
    if let Some(command) = command {
        let _ = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(work_dir)
            .output();
    }
}
