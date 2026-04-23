use crate::cli::{
    abbreviate_oid, format_current_location, format_current_pair, format_resolved_pair,
    format_side_anchored, format_status, highest_status, parse_stale_detail, parse_stale_format,
    print_indented_message, sorted_links, status_rank, StaleDetail, StaleFormat,
};
use anyhow::{Result, anyhow};
use git_mesh_legacy::{
    is_ancestor_commit, list_mesh_names, mesh_commit_info, read_git_text, resolve_commit_ish,
    stale_mesh, CulpritCommit, LinkResolved, LinkStatus, MeshCommitInfo, MeshResolved,
};
use serde::Serialize;
use std::fs;
use std::process::Command as ProcessCommand;

#[derive(Clone, Debug)]
pub(crate) struct StaleMeshReport {
    pub mesh: MeshResolved,
    pub info: MeshCommitInfo,
}

pub(crate) fn run_stale(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<i32> {
    let detail = parse_stale_detail(sub_matches)?;
    let format = parse_stale_format(
        sub_matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human"),
    )?;
    let since = sub_matches
        .get_one::<String>("since")
        .map(|value| resolve_commit_ish(repo, value))
        .transpose()?;
    let reports = load_stale_reports(repo, sub_matches.get_one::<String>("name"), since.as_deref())?;

    match format {
        StaleFormat::Human => print_human_stale(repo, &reports, detail)?,
        StaleFormat::Porcelain => print_porcelain_stale(&reports),
        StaleFormat::Json => print_json_stale(&reports)?,
        StaleFormat::Junit => print_junit_stale(&reports)?,
        StaleFormat::GitHubActions => print_github_actions_stale(&reports),
    }

    if sub_matches.get_flag("exit-code") && reports_have_stale(&reports) {
        return Ok(1);
    }
    Ok(0)
}

fn load_stale_reports(
    repo: &gix::Repository,
    name: Option<&String>,
    since: Option<&str>,
) -> Result<Vec<StaleMeshReport>> {
    let mut names = match name {
        Some(name) => vec![name.clone()],
        None => list_mesh_names(repo)?,
    };
    names.sort();

    let mut reports = Vec::with_capacity(names.len());
    for mesh_name in names {
        let mut mesh = stale_mesh(repo, &mesh_name)?;
        if let Some(since) = since {
            let mut filtered = Vec::with_capacity(mesh.links.len());
            for link in mesh.links {
                if is_ancestor_commit(repo, since, &link.anchor_sha)? {
                    filtered.push(link);
                }
            }
            mesh.links = filtered;
        }
        reports.push(StaleMeshReport {
            info: mesh_commit_info(repo, &mesh_name)?,
            mesh,
        });
    }

    reports.sort_by_key(|report| {
        (
            std::cmp::Reverse(highest_status(&report.mesh)),
            report.mesh.name.clone(),
        )
    });
    Ok(reports)
}

pub(crate) fn print_human_stale(
    repo: &gix::Repository,
    reports: &[StaleMeshReport],
    detail: StaleDetail,
) -> Result<()> {
    for (index, report) in reports.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_stale(repo, &report.mesh, &report.info, detail)?;
    }
    Ok(())
}

fn print_stale(
    repo: &gix::Repository,
    mesh: &MeshResolved,
    info: &MeshCommitInfo,
    detail: StaleDetail,
) -> Result<()> {
    println!("mesh {}", mesh.name);
    println!("commit {}", info.commit_oid);
    println!("Author: {} <{}>", info.author_name, info.author_email);
    println!("Date:   {}", info.author_date);
    println!();
    print_indented_message(&mesh.message);
    println!();

    let stale_count = mesh
        .links
        .iter()
        .filter(|link| link.status != LinkStatus::Fresh)
        .count();
    println!("{stale_count} stale of {} links:", mesh.links.len());

    let mut links = mesh.links.clone();
    links.sort_by_key(|link| std::cmp::Reverse(status_rank(link.status)));
    for link in &links {
        println!();
        match detail {
            StaleDetail::Oneline => println!(
                "  {:<10} {}",
                format_status(link.status),
                format_resolved_pair(link)?
            ),
            StaleDetail::Stat => println!(
                "  {:<10} {}  {} -> {}",
                format_status(link.status),
                abbreviate_oid(&link.anchor_sha),
                format_resolved_pair(link)?,
                format_current_pair(link)
            ),
            StaleDetail::Full | StaleDetail::Patch => {
                println!(
                    "  {:<10} {}  {}",
                    format_status(link.status),
                    abbreviate_oid(&link.anchor_sha),
                    format_resolved_pair(link)?
                );
                let last_index = link.sides.len().saturating_sub(1);
                for (index, side) in link.sides.iter().enumerate() {
                    let branch = if index == last_index { "└─" } else { "├─" };
                    println!(
                        "             {branch} {:<10} {}",
                        format_status(side.status),
                        format_human_side_summary(repo, side)?
                    );
                    if let Some(culprit) = &side.culprit {
                        let relative = culprit
                            .committed_at
                            .map(|ts| format!("  ({})", format_relative_time(ts, now_seconds())))
                            .unwrap_or_default();
                        println!(
                            "                caused by {} {}{relative}",
                            abbreviate_oid(&culprit.commit_oid),
                            culprit.summary
                        );
                    }
                    if detail == StaleDetail::Patch
                        && side.status != LinkStatus::Fresh
                        && let Some(patch) = render_side_patch(repo, side)?
                    {
                        for line in patch.lines() {
                            println!("                {line}");
                        }
                    }
                }
                if link.status != LinkStatus::Fresh {
                    println!();
                    println!("             reconcile with:");
                    for line in format_wrapped_reconcile(&link.reconcile_command) {
                        println!("               {line}");
                    }
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn print_porcelain_stale(reports: &[StaleMeshReport]) {
    for report in reports {
        for link in sorted_links(&report.mesh) {
            let anchored_pair = format_resolved_pair(&link).expect("format pair");
            let current_pair = format_current_pair(&link);
            println!(
                "mesh={}\tcommit={}\tstatus={}\tanchor={}\tpair={}\tcurrentPair={}\tlinkId={}\treconcile={}\tleftCulprit={}\trightCulprit={}",
                report.mesh.name,
                report.info.commit_oid,
                format_status(link.status),
                link.anchor_sha,
                anchored_pair,
                current_pair,
                link.link_id,
                shell_escape(&link.reconcile_command),
                format_culprit_field(link.sides[0].culprit.as_ref()),
                format_culprit_field(link.sides[1].culprit.as_ref()),
            );
        }
    }
}

/// Emit `--format=json` output in the shape pinned by
/// docs/git-mesh.md §10.4: each mesh report is a versioned object
/// `{"version": 1, "mesh", "commit", "links": [...]}` and each link entry
/// mirrors the LSP `Diagnostic` shape (`severity`, `range`, `message`,
/// `code`, `data.culprit`, `data.reconcile_command`) so editor plugins can
/// surface it directly as squiggles + quick-fixes. With a single `<name>`
/// the top level is one such object; with no name the output is a JSON
/// array of them (worst-first, matching the human output's ordering).
pub(crate) fn print_json_stale(reports: &[StaleMeshReport]) -> Result<()> {
    #[derive(Serialize)]
    struct JsonMesh<'a> {
        version: u32,
        mesh: &'a str,
        commit: &'a str,
        stale_count: usize,
        link_count: usize,
        links: Vec<serde_json::Value>,
    }

    let payloads: Vec<JsonMesh> = reports
        .iter()
        .map(|report| JsonMesh {
            version: 1,
            mesh: &report.mesh.name,
            commit: &report.info.commit_oid,
            stale_count: report
                .mesh
                .links
                .iter()
                .filter(|link| link.status != LinkStatus::Fresh)
                .count(),
            link_count: report.mesh.links.len(),
            links: sorted_links(&report.mesh)
                .into_iter()
                .map(link_to_diagnostic)
                .collect(),
        })
        .collect();

    let rendered = if payloads.len() == 1 {
        serde_json::to_string_pretty(&payloads[0])?
    } else {
        serde_json::to_string_pretty(&payloads)?
    };
    println!("{rendered}");
    Ok(())
}

/// Pick the LSP `DiagnosticSeverity` integer (1=Error, 2=Warning, 3=Info,
/// 4=Hint) for a `LinkStatus`. Worst statuses (`ORPHANED`, `MISSING`,
/// `REWRITTEN`) map to Error; `MODIFIED` to Warning; `MOVED` to Info;
/// `FRESH` to Hint. This matches §10.4's intent that editors surface
/// non-fresh links as actionable diagnostics.
fn severity_for(status: LinkStatus) -> u8 {
    match status {
        LinkStatus::Orphaned | LinkStatus::Missing | LinkStatus::Rewritten => 1,
        LinkStatus::Modified => 2,
        LinkStatus::Moved => 3,
        LinkStatus::Fresh => 4,
    }
}

/// Build an LSP `Range` from a 1-based inclusive git-mesh line range. LSP
/// positions are 0-based and ranges are end-exclusive, so a mesh range
/// `L<start>-L<end>` becomes lines `[start-1, end)` spanning the full
/// character width of each line.
fn lsp_range(start: u32, end: u32) -> LspRange {
    LspRange {
        start: Position {
            line: start.saturating_sub(1),
            character: 0,
        },
        end: Position {
            line: end,
            character: 0,
        },
    }
}

#[derive(Serialize)]
struct Position {
    line: u32,
    character: u32,
}

#[derive(Serialize)]
struct LspRange {
    start: Position,
    end: Position,
}

fn side_diagnostic(side: &git_mesh_legacy::SideResolved) -> serde_json::Value {
    let location = side.current.as_ref().unwrap_or(&side.anchored);
    serde_json::json!({
        "path": location.path,
        "range": lsp_range(location.start, location.end),
        "status": side.status,
        "culprit": side.culprit,
    })
}

fn link_to_diagnostic(link: LinkResolved) -> serde_json::Value {
    let primary = link.sides[0]
        .current
        .as_ref()
        .unwrap_or(&link.sides[0].anchored);
    let message = format!(
        "{} {} \u{2192} {}",
        format_status(link.status),
        format_resolved_pair(&link).unwrap_or_default(),
        format_current_pair(&link)
    );
    let culprit = link
        .sides
        .iter()
        .find_map(|side| side.culprit.clone());

    serde_json::json!({
        "severity": severity_for(link.status),
        "range": lsp_range(primary.start, primary.end),
        "message": message,
        "code": format_status(link.status),
        "source": "git-mesh",
        "data": {
            "link_id": link.link_id,
            "anchor_sha": link.anchor_sha,
            "pair": format_resolved_pair(&link).unwrap_or_default(),
            "current_pair": format_current_pair(&link),
            "reconcile_command": link.reconcile_command,
            "sides": [
                side_diagnostic(&link.sides[0]),
                side_diagnostic(&link.sides[1]),
            ],
            "culprit": culprit,
        },
    })
}

pub(crate) fn print_junit_stale(reports: &[StaleMeshReport]) -> Result<()> {
    let tests = reports
        .iter()
        .map(|report| report.mesh.links.len())
        .sum::<usize>();
    let failures = reports
        .iter()
        .flat_map(|report| report.mesh.links.iter())
        .filter(|link| link.status != LinkStatus::Fresh)
        .count();

    println!("<testsuite name=\"git-mesh stale\" tests=\"{tests}\" failures=\"{failures}\">");
    for report in reports {
        for link in sorted_links(&report.mesh) {
            let pair = xml_escape(&format_resolved_pair(&link)?);
            let name = xml_escape(&format!("{} {}", report.mesh.name, pair));
            println!(
                "  <testcase classname=\"{}\" name=\"{}\">",
                xml_escape(&report.mesh.name),
                name
            );
            if link.status != LinkStatus::Fresh {
                let message = xml_escape(&format!(
                    "{} {} -> {}",
                    format_status(link.status),
                    format_resolved_pair(&link)?,
                    format_current_pair(&link)
                ));
                println!("    <failure message=\"{message}\">");
                println!("{}", xml_escape(&link.reconcile_command));
                println!("    </failure>");
            }
            println!("  </testcase>");
        }
    }
    println!("</testsuite>");
    Ok(())
}

pub(crate) fn print_github_actions_stale(reports: &[StaleMeshReport]) {
    for report in reports {
        for link in sorted_links(&report.mesh) {
            if link.status == LinkStatus::Fresh {
                continue;
            }
            let culprit = link
                .sides
                .iter()
                .find_map(|side| side.culprit.as_ref())
                .map(|culprit| format!(" ({})", culprit.summary))
                .unwrap_or_default();
            let message = github_actions_escape(&format!(
                "mesh {}: {} {} -> {}{}",
                report.mesh.name,
                format_status(link.status),
                format_resolved_pair(&link).unwrap_or_default(),
                format_current_pair(&link),
                culprit
            ));
            println!(
                "::warning file={},line={}::{message}",
                github_actions_escape(&link.sides[0].anchored.path),
                link.sides[0].anchored.start
            );
        }
    }
}

/// Format the per-side summary for `git mesh stale` human output. The doc's
/// §10.4 example uses a Unicode right arrow (`→`) between the anchored range
/// and the current location, drops the path when it has not changed, and
/// appends a human hint describing why the side is stale. `MOVED` gets
/// `(file unchanged, lines shifted)`; `MODIFIED`/`REWRITTEN` get
/// `(<changed>/<total> lines rewritten)` computed from the blob texts.
pub(crate) fn format_human_side_summary(
    repo: &gix::Repository,
    side: &git_mesh_legacy::SideResolved,
) -> Result<String> {
    let anchored = format_side_anchored(side);

    let Some(current) = side.current.as_ref() else {
        return Ok(anchored);
    };

    let path_changed = current.path != side.anchored.path;
    let range_changed = current.start != side.anchored.start || current.end != side.anchored.end;

    let hint = match side.status {
        LinkStatus::Moved => Some("file unchanged, lines shifted".to_string()),
        LinkStatus::Modified | LinkStatus::Rewritten => {
            Some(format_rewritten_hint(repo, side, current)?)
        }
        _ => None,
    };

    // Fresh side with no movement: just the anchored range.
    if !path_changed && !range_changed && hint.is_none() {
        return Ok(anchored);
    }

    let shifted_suffix = if path_changed {
        Some(format_current_location(current))
    } else if range_changed {
        Some(format!("L{}-L{}", current.start, current.end))
    } else {
        None
    };

    Ok(match (shifted_suffix, hint) {
        (Some(shifted), Some(hint)) => format!("{anchored} \u{2192} {shifted}  ({hint})"),
        (Some(shifted), None) => format!("{anchored} \u{2192} {shifted}"),
        (None, Some(hint)) => format!("{anchored}  ({hint})"),
        (None, None) => anchored,
    })
}

pub(crate) fn format_rewritten_hint(
    repo: &gix::Repository,
    side: &git_mesh_legacy::SideResolved,
    current: &git_mesh_legacy::LinkLocation,
) -> Result<String> {
    let anchored_text = slice_blob_lines(
        &read_git_text(repo, &side.anchored.blob)?,
        side.anchored.start,
        side.anchored.end,
    )?;
    let current_text = slice_blob_lines(
        &read_git_text(repo, &current.blob)?,
        current.start,
        current.end,
    )?;

    let anchored_lines: Vec<&str> = anchored_text.lines().collect();
    let current_lines: Vec<&str> = current_text.lines().collect();
    let total = anchored_lines.len().max(current_lines.len());
    let mut changed = 0usize;
    for idx in 0..total {
        let a = anchored_lines.get(idx);
        let c = current_lines.get(idx);
        if a != c {
            changed += 1;
        }
    }

    Ok(format!("{changed}/{total} lines rewritten"))
}

/// Break the single-line reconcile command into the multi-line wrapped form
/// shown in docs/git-mesh.md §10.4: the `git mesh commit <name>` head stays on
/// one line, and each subsequent `--unlink`, `--link`, and `-m` argument moves
/// to its own continuation line. Continuation lines after the first are
/// indented four spaces past `git mesh commit` to mirror the doc example.
pub(crate) fn format_wrapped_reconcile(command: &str) -> Vec<String> {
    // Split the canonical single-line reconcile command into head + per-flag
    // segments (`--unlink ...`, `--link ...`, `-m ...`). Tokens are known to
    // be whitespace-separated because `build_reconcile_command` emits them
    // that way.
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for token in command.split(' ') {
        if matches!(token, "--unlink" | "--link" | "-m") && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(token);
    }
    if !current.is_empty() {
        parts.push(current);
    }

    if parts.len() <= 1 {
        return parts;
    }

    let mut wrapped = Vec::with_capacity(parts.len());
    let last = parts.len() - 1;
    for (index, part) in parts.into_iter().enumerate() {
        if index == 0 {
            wrapped.push(format!("{part} \\"));
        } else if index == last {
            wrapped.push(format!("    {part}"));
        } else {
            wrapped.push(format!("    {part} \\"));
        }
    }
    wrapped
}

/// Render a committer timestamp as a short relative phrase matching the
/// `(2 days ago)` style shown in docs/git-mesh.md §10.4. Future timestamps
/// collapse to `"just now"`; precision coarsens from seconds to years as the
/// gap widens.
pub(crate) fn format_relative_time(committed_at: i64, now: i64) -> String {
    let delta = now.saturating_sub(committed_at);
    if delta <= 0 {
        return "just now".to_string();
    }
    let (value, unit) = if delta < 60 {
        (delta, "second")
    } else if delta < 3_600 {
        (delta / 60, "minute")
    } else if delta < 86_400 {
        (delta / 3_600, "hour")
    } else if delta < 2_592_000 {
        (delta / 86_400, "day")
    } else if delta < 31_536_000 {
        (delta / 2_592_000, "month")
    } else {
        (delta / 31_536_000, "year")
    };
    let plural = if value == 1 { "" } else { "s" };
    format!("{value} {unit}{plural} ago")
}

pub(crate) fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn format_culprit_field(culprit: Option<&CulpritCommit>) -> String {
    culprit
        .map(|culprit| format!("{} {}", culprit.commit_oid, culprit.summary))
        .unwrap_or_default()
}

fn shell_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\t', "\\t")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn github_actions_escape(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace(':', "%3A")
        .replace(',', "%2C")
}

fn reports_have_stale(reports: &[StaleMeshReport]) -> bool {
    reports.iter().any(|report| {
        report
            .mesh
            .links
            .iter()
            .any(|link| link.status != LinkStatus::Fresh)
    })
}

fn render_side_patch(
    repo: &gix::Repository,
    side: &git_mesh_legacy::SideResolved,
) -> Result<Option<String>> {
    let current = match &side.current {
        Some(current) if side.status != LinkStatus::Moved => current,
        _ => return Ok(None),
    };

    let anchored_text = slice_blob_lines(
        &read_git_text(repo, &side.anchored.blob)?,
        side.anchored.start,
        side.anchored.end,
    )?;
    let current_text = slice_blob_lines(
        &read_git_text(repo, &current.blob)?,
        current.start,
        current.end,
    )?;

    if anchored_text == current_text {
        return Ok(None);
    }

    let base = std::env::temp_dir().join(format!("git-mesh-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base)?;
    let old_path = base.join("anchored.txt");
    let new_path = base.join("current.txt");
    fs::write(&old_path, anchored_text)?;
    fs::write(&new_path, current_text)?;

    let output = ProcessCommand::new("git")
        .current_dir(
            repo.workdir()
                .ok_or_else(|| anyhow!("Bare repositories are not supported"))?,
        )
        .args([
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--unified=3",
            old_path
                .to_str()
                .ok_or_else(|| anyhow!("temporary file path is not valid UTF-8"))?,
            new_path
                .to_str()
                .ok_or_else(|| anyhow!("temporary file path is not valid UTF-8"))?,
        ])
        .output()?;

    let _ = fs::remove_file(&old_path);
    let _ = fs::remove_file(&new_path);
    let _ = fs::remove_dir(&base);

    match output.status.code() {
        Some(0) => Ok(None),
        Some(1) => Ok(Some(rewrite_patch_labels(
            &String::from_utf8(output.stdout)?,
            &format_side_anchored(side),
            &format_current_location(current),
        ))),
        _ => Err(anyhow!(
            "git diff --no-index failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )),
    }
}

fn slice_blob_lines(text: &str, start: u32, end: u32) -> Result<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start_index = start
        .checked_sub(1)
        .ok_or_else(|| anyhow!("range start must be at least 1"))? as usize;
    let end_index = end as usize;
    let slice = lines
        .get(start_index..end_index)
        .ok_or_else(|| anyhow!("range {start}..={end} is out of bounds"))?;
    let mut rendered = slice.join("\n");
    rendered.push('\n');
    Ok(rendered)
}

fn rewrite_patch_labels(diff: &str, anchored_label: &str, current_label: &str) -> String {
    let mut lines = diff.lines();
    let mut rewritten = Vec::new();

    if lines.next().is_some() {
        rewritten.push(format!("--- {anchored_label}"));
    }
    if lines.next().is_some() {
        rewritten.push(format!("+++ {current_label}"));
    }
    rewritten.extend(lines.map(str::to_string));

    let mut output = rewritten.join("\n");
    output.push('\n');
    output
}
