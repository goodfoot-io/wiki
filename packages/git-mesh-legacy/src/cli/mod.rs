pub mod commit;
pub mod show;
pub mod stale_output;
pub mod structural;
pub mod sync;

use anyhow::{Result, anyhow};
use git_mesh_legacy::{CopyDetection, LinkResolved, LinkStatus, MeshResolved, RangeSpec, SideSpec, StoredLink};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(crate) struct PrintOptions {
    pub oneline: bool,
    pub no_abbrev: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StaleFormat {
    Human,
    Porcelain,
    Json,
    Junit,
    GitHubActions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StaleDetail {
    Full,
    Oneline,
    Stat,
    Patch,
}

pub(crate) fn parse_stale_format(value: &str) -> Result<StaleFormat> {
    match value {
        "human" => Ok(StaleFormat::Human),
        "porcelain" => Ok(StaleFormat::Porcelain),
        "json" => Ok(StaleFormat::Json),
        "junit" => Ok(StaleFormat::Junit),
        "github-actions" => Ok(StaleFormat::GitHubActions),
        _ => Err(anyhow!("invalid stale format `{value}`")),
    }
}

pub(crate) fn parse_stale_detail(matches: &clap::ArgMatches) -> Result<StaleDetail> {
    let detail = if matches.get_flag("patch") {
        StaleDetail::Patch
    } else if matches.get_flag("stat") {
        StaleDetail::Stat
    } else if matches.get_flag("oneline") {
        StaleDetail::Oneline
    } else {
        StaleDetail::Full
    };
    Ok(detail)
}

pub(crate) fn parse_link_pair(
    text: &str,
    copy_detection: Option<CopyDetection>,
    ignore_whitespace: Option<bool>,
) -> Result<[SideSpec; 2]> {
    let [left, right] = split_link_pair(text)?;
    Ok([
        into_side_spec(parse_range(left)?, copy_detection, ignore_whitespace),
        into_side_spec(parse_range(right)?, copy_detection, ignore_whitespace),
    ])
}

pub(crate) fn parse_range_pair(text: &str) -> Result<[RangeSpec; 2]> {
    let [left, right] = split_link_pair(text)?;
    Ok([parse_range(left)?, parse_range(right)?])
}

fn split_link_pair(text: &str) -> Result<[&str; 2]> {
    let (left, right) = text
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid link pair `{text}`; expected <rangeA>:<rangeB>"))?;
    anyhow::ensure!(
        !left.is_empty() && !right.is_empty(),
        "invalid link pair `{text}`; expected <rangeA>:<rangeB>"
    );
    Ok([left, right])
}

pub(crate) fn parse_range(text: &str) -> Result<RangeSpec> {
    let (path, fragment) = text
        .split_once("#L")
        .ok_or_else(|| anyhow!("invalid range `{text}`; expected <path>#L<start>-L<end>"))?;
    let (start, end) = fragment
        .split_once("-L")
        .ok_or_else(|| anyhow!("invalid range `{text}`; expected <path>#L<start>-L<end>"))?;
    anyhow::ensure!(!path.is_empty(), "range path cannot be empty");

    let start: u32 = start.parse()?;
    let end: u32 = end.parse()?;
    anyhow::ensure!(start >= 1, "range start must be at least 1");
    anyhow::ensure!(end >= start, "range end must be at least start");

    Ok(RangeSpec {
        path: path.to_string(),
        start,
        end,
    })
}

fn into_side_spec(
    range: RangeSpec,
    copy_detection: Option<CopyDetection>,
    ignore_whitespace: Option<bool>,
) -> SideSpec {
    SideSpec {
        path: range.path,
        start: range.start,
        end: range.end,
        copy_detection,
        ignore_whitespace,
    }
}

pub(crate) fn parse_copy_detection(text: &str) -> Result<CopyDetection> {
    match text {
        "off" => Ok(CopyDetection::Off),
        "same-commit" => Ok(CopyDetection::SameCommit),
        "any-file-in-commit" => Ok(CopyDetection::AnyFileInCommit),
        "any-file-in-repo" => Ok(CopyDetection::AnyFileInRepo),
        _ => Err(anyhow!("invalid copy detection `{text}`")),
    }
}

pub(crate) fn print_indented_message(message: &str) {
    for line in message.lines() {
        println!("    {line}");
    }
}

pub(crate) fn format_resolved_pair(link: &LinkResolved) -> Result<String> {
    Ok(format!(
        "{}:{}",
        format_range_spec(&RangeSpec {
            path: link.sides[0].anchored.path.clone(),
            start: link.sides[0].anchored.start,
            end: link.sides[0].anchored.end,
        }),
        format_range_spec(&RangeSpec {
            path: link.sides[1].anchored.path.clone(),
            start: link.sides[1].anchored.start,
            end: link.sides[1].anchored.end,
        })
    ))
}

pub(crate) fn format_current_location(location: &git_mesh_legacy::LinkLocation) -> String {
    format!("{}#L{}-L{}", location.path, location.start, location.end)
}

pub(crate) fn format_side_anchored(side: &git_mesh_legacy::SideResolved) -> String {
    format_range_spec(&RangeSpec {
        path: side.anchored.path.clone(),
        start: side.anchored.start,
        end: side.anchored.end,
    })
}

pub(crate) fn format_current_pair(link: &LinkResolved) -> String {
    format!(
        "{}:{}",
        link.sides[0]
            .current
            .as_ref()
            .map(format_current_location)
            .unwrap_or_else(|| format_side_anchored(&link.sides[0])),
        link.sides[1]
            .current
            .as_ref()
            .map(format_current_location)
            .unwrap_or_else(|| format_side_anchored(&link.sides[1]))
    )
}

pub(crate) fn format_range_spec(range: &RangeSpec) -> String {
    format!("{}#L{}-L{}", range.path, range.start, range.end)
}

pub(crate) fn format_stored_side(side: &git_mesh_legacy::LinkSide) -> String {
    format_range_spec(&RangeSpec {
        path: side.path.clone(),
        start: side.start,
        end: side.end,
    })
}

pub(crate) fn format_link_pair(link: &StoredLink) -> String {
    format!(
        "{}:{}",
        format_stored_side(&link.sides[0]),
        format_stored_side(&link.sides[1])
    )
}

pub(crate) fn stored_links_sorted(links: &[StoredLink]) -> Vec<&StoredLink> {
    let mut ordered = links.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|link| {
        (
            format_link_pair(link),
            link.anchor_sha.clone(),
            link.id.clone(),
        )
    });
    ordered
}

pub(crate) fn index_links_by_pair(links: &[StoredLink]) -> BTreeMap<String, &StoredLink> {
    links
        .iter()
        .map(|link| (format_link_pair(link), link))
        .collect()
}

pub(crate) fn maybe_abbreviate(oid: &str, no_abbrev: bool) -> &str {
    if no_abbrev { oid } else { abbreviate_oid(oid) }
}

pub(crate) fn abbreviate_oid(oid: &str) -> &str {
    let end = oid.len().min(8);
    &oid[..end]
}

pub(crate) fn format_status(status: LinkStatus) -> &'static str {
    match status {
        LinkStatus::Fresh => "FRESH",
        LinkStatus::Moved => "MOVED",
        LinkStatus::Modified => "MODIFIED",
        LinkStatus::Rewritten => "REWRITTEN",
        LinkStatus::Missing => "MISSING",
        LinkStatus::Orphaned => "ORPHANED",
    }
}

pub(crate) fn status_rank(status: LinkStatus) -> u8 {
    match status {
        LinkStatus::Orphaned => 5,
        LinkStatus::Missing => 4,
        LinkStatus::Rewritten => 3,
        LinkStatus::Modified => 2,
        LinkStatus::Moved => 1,
        LinkStatus::Fresh => 0,
    }
}

pub(crate) fn highest_status(mesh: &MeshResolved) -> u8 {
    mesh.links
        .iter()
        .map(|link| status_rank(link.status))
        .max()
        .unwrap_or(0)
}

pub(crate) fn sorted_links(mesh: &MeshResolved) -> Vec<LinkResolved> {
    let mut links = mesh.links.clone();
    links.sort_by_key(|link| {
        (
            std::cmp::Reverse(status_rank(link.status)),
            format_resolved_pair(link).unwrap_or_default(),
            link.link_id.clone(),
        )
    });
    links
}
