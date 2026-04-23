use crate::git::{
    apply_ref_transaction, git_stdout, git_stdout_raw, git_with_input, RefUpdate,
};
use crate::types::*;
use anyhow::{Result, anyhow};
use chrono::Utc;
use std::path::Path;
use uuid::Uuid;

pub fn create_link(repo: &gix::Repository, input: CreateLinkInput) -> Result<(String, Link)> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let anchor_sha = match input.anchor_sha {
        Some(anchor_sha) => anchor_sha,
        None => git_stdout(work_dir, ["rev-parse", "HEAD"])?,
    };
    let id = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let link = build_link(work_dir, &anchor_sha, input.sides)?;
    let blob_oid = write_link_blob(work_dir, &link)?;
    apply_ref_transaction(
        work_dir,
        &[RefUpdate::Create {
            name: format!("refs/links/v1/{id}"),
            new_oid: blob_oid,
        }],
    )?;
    Ok((id, link))
}

pub(crate) fn build_link_side(
    work_dir: &std::path::Path,
    anchor_sha: &str,
    side: SideSpec,
) -> Result<LinkSide> {
    let blob = resolve_side_blob(work_dir, anchor_sha, &side)?;
    validate_side_range(work_dir, &blob, &side)?;

    Ok(LinkSide {
        path: side.path.clone(),
        start: side.start,
        end: side.end,
        blob,
        copy_detection: side.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
        ignore_whitespace: side.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
    })
}

pub(crate) fn build_link(work_dir: &Path, anchor_sha: &str, sides: [SideSpec; 2]) -> Result<Link> {
    let [left, right] = normalize_side_specs(sides);
    let mut sides = [
        build_link_side(work_dir, anchor_sha, left)?,
        build_link_side(work_dir, anchor_sha, right)?,
    ];
    sides.sort();
    Ok(Link {
        anchor_sha: anchor_sha.to_string(),
        created_at: Utc::now().to_rfc3339(),
        sides,
    })
}

fn resolve_side_blob(
    work_dir: &std::path::Path,
    anchor_sha: &str,
    side: &SideSpec,
) -> Result<String> {
    git_stdout(
        work_dir,
        ["rev-parse", &format!("{anchor_sha}:{}", side.path)],
    )
}

pub(crate) fn validate_side_range(
    work_dir: &std::path::Path,
    blob: &str,
    side: &SideSpec,
) -> Result<()> {
    anyhow::ensure!(side.start >= 1, "range start must be at least 1");
    anyhow::ensure!(side.end >= side.start, "range end must be at least start");

    let line_count = git_stdout(work_dir, ["cat-file", "-p", blob])?
        .lines()
        .count() as u32;
    anyhow::ensure!(
        side.end <= line_count,
        "range {}..={} is out of bounds for {} lines in {}",
        side.start,
        side.end,
        line_count,
        side.path
    );

    Ok(())
}

pub fn read_link(repo: &gix::Repository, id: &str) -> Result<Link> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    read_link_from_ref(work_dir, id)
}

pub fn serialize_link(link: &Link) -> String {
    format!(
        "anchor {}\ncreated {}\nside {} {} {} {} {}\t{}\nside {} {} {} {} {}\t{}\n",
        link.anchor_sha,
        link.created_at,
        link.sides[0].start,
        link.sides[0].end,
        link.sides[0].blob,
        serialize_copy_detection(link.sides[0].copy_detection),
        link.sides[0].ignore_whitespace,
        link.sides[0].path,
        link.sides[1].start,
        link.sides[1].end,
        link.sides[1].blob,
        serialize_copy_detection(link.sides[1].copy_detection),
        link.sides[1].ignore_whitespace,
        link.sides[1].path
    )
}

pub fn parse_link(text: &str) -> Result<Link> {
    // §4.1 on-disk format:
    //   anchor <sha>
    //   created <iso-8601>
    //   side <start> <end> <blob> <copy-detection> <ignore-whitespace>\t<path>
    //   side <start> <end> <blob> <copy-detection> <ignore-whitespace>\t<path>
    //
    // - Trailing newline, no blank lines.
    // - Headers are `key SP value\n`. Unknown headers are tolerated (additive
    //   extensions). The two `side` lines are sorted on write.
    anyhow::ensure!(
        text.ends_with('\n'),
        "link blob must end with a trailing newline"
    );
    anyhow::ensure!(!text.is_empty(), "link blob must not be empty");

    let mut anchor_sha: Option<String> = None;
    let mut created_at: Option<String> = None;
    let mut sides: Vec<LinkSide> = Vec::with_capacity(2);
    let mut seen_side = false;

    for (idx, line) in text.lines().enumerate() {
        anyhow::ensure!(
            !line.is_empty(),
            "link blob must not contain blank lines (line {})",
            idx + 1
        );

        if let Some(rest) = line.strip_prefix("anchor ") {
            anyhow::ensure!(
                !seen_side,
                "`anchor` header must precede `side` lines (line {})",
                idx + 1
            );
            anyhow::ensure!(
                anchor_sha.is_none(),
                "duplicate `anchor` header (line {})",
                idx + 1
            );
            anyhow::ensure!(
                !rest.is_empty(),
                "`anchor` header has empty value (line {})",
                idx + 1
            );
            anchor_sha = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("created ") {
            anyhow::ensure!(
                !seen_side,
                "`created` header must precede `side` lines (line {})",
                idx + 1
            );
            anyhow::ensure!(
                created_at.is_none(),
                "duplicate `created` header (line {})",
                idx + 1
            );
            anyhow::ensure!(
                !rest.is_empty(),
                "`created` header has empty value (line {})",
                idx + 1
            );
            created_at = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("side ") {
            seen_side = true;
            anyhow::ensure!(
                sides.len() < 2,
                "link must contain exactly two `side` lines (line {})",
                idx + 1
            );
            let (meta, path) = rest
                .split_once('\t')
                .ok_or_else(|| anyhow!("`side` line is missing TAB before path (line {})", idx + 1))?;
            anyhow::ensure!(
                !path.is_empty(),
                "`side` line has empty path (line {})",
                idx + 1
            );
            let fields: Vec<&str> = meta.split(' ').collect();
            anyhow::ensure!(
                fields.len() == 5,
                "`side` line must have exactly 5 space-separated fields before TAB (line {})",
                idx + 1
            );
            let start: u32 = fields[0]
                .parse()
                .map_err(|_| anyhow!("invalid side start `{}` (line {})", fields[0], idx + 1))?;
            let end: u32 = fields[1]
                .parse()
                .map_err(|_| anyhow!("invalid side end `{}` (line {})", fields[1], idx + 1))?;
            let blob = fields[2].to_string();
            anyhow::ensure!(
                !blob.is_empty(),
                "`side` line has empty blob (line {})",
                idx + 1
            );
            let copy_detection = match fields[3] {
                "off" => CopyDetection::Off,
                "same-commit" => CopyDetection::SameCommit,
                "any-file-in-commit" => CopyDetection::AnyFileInCommit,
                "any-file-in-repo" => CopyDetection::AnyFileInRepo,
                other => anyhow::bail!(
                    "invalid copy detection `{}` (line {})",
                    other,
                    idx + 1
                ),
            };
            let ignore_whitespace = match fields[4] {
                "true" => true,
                "false" => false,
                other => anyhow::bail!(
                    "invalid ignore_whitespace `{}` (line {})",
                    other,
                    idx + 1
                ),
            };

            sides.push(LinkSide {
                path: path.to_string(),
                start,
                end,
                blob,
                copy_detection,
                ignore_whitespace,
            });
            continue;
        }
        // Unknown headers are tolerated so additive extensions don't break v1
        // readers. Reject lines that don't match the `key SP value` shape at
        // all, though — those aren't extension headers, they're corruption.
        anyhow::ensure!(
            line.split_once(' ').is_some_and(|(k, _)| !k.is_empty()),
            "malformed line `{}` (line {})",
            line,
            idx + 1
        );
    }

    anyhow::ensure!(
        sides.len() == 2,
        "link must contain exactly two `side` lines (found {})",
        sides.len()
    );

    // Writers must canonicalize, but tolerate mis-ordered input from older
    // blobs by re-sorting on read.
    sides.sort();
    let [left, right]: [LinkSide; 2] = sides
        .try_into()
        .map_err(|_| anyhow!("link must contain exactly two sides"))?;

    Ok(Link {
        anchor_sha: anchor_sha.ok_or_else(|| anyhow!("missing `anchor` header"))?,
        created_at: created_at.ok_or_else(|| anyhow!("missing `created` header"))?,
        sides: [left, right],
    })
}

pub(crate) fn write_link_blob(work_dir: &Path, link: &Link) -> Result<String> {
    git_with_input(
        work_dir,
        ["hash-object", "-w", "--stdin"],
        &serialize_link(link),
    )
}

pub(crate) fn read_link_from_ref(work_dir: &Path, id: &str) -> Result<Link> {
    let link_oid = git_stdout(work_dir, ["rev-parse", &format!("refs/links/v1/{id}")])?;
    // §4.1 requires a trailing newline on the stored blob. `git_stdout`
    // trims output, so use the raw variant here to feed `parse_link` the
    // exact on-disk bytes and let its invariants run.
    let link_text = git_stdout_raw(work_dir, ["cat-file", "-p", &link_oid])?;
    parse_link(&link_text)
}

pub(crate) fn serialize_copy_detection(copy_detection: CopyDetection) -> &'static str {
    match copy_detection {
        CopyDetection::Off => "off",
        CopyDetection::SameCommit => "same-commit",
        CopyDetection::AnyFileInCommit => "any-file-in-commit",
        CopyDetection::AnyFileInRepo => "any-file-in-repo",
    }
}

pub(crate) fn normalize_side_specs(mut sides: [SideSpec; 2]) -> [SideSpec; 2] {
    for side in &mut sides {
        side.copy_detection.get_or_insert(DEFAULT_COPY_DETECTION);
        side.ignore_whitespace
            .get_or_insert(DEFAULT_IGNORE_WHITESPACE);
    }

    sides.sort_by(|a, b| {
        (
            &a.path,
            a.start,
            a.end,
            a.copy_detection,
            a.ignore_whitespace,
        )
            .cmp(&(
                &b.path,
                b.start,
                b.end,
                b.copy_detection,
                b.ignore_whitespace,
            ))
    });
    sides
}

pub(crate) fn normalize_range_specs(mut sides: [RangeSpec; 2]) -> [RangeSpec; 2] {
    sides.sort();
    sides
}
