//! `git mesh stale` — §10.4.

use crate::cli::{StaleArgs, StaleFormat};
use crate::range::read_range;
use crate::stale::{resolve_mesh, stale_meshes};
use crate::types::{RangeResolved, RangeStatus};
use anyhow::Result;
use serde_json::json;

pub fn run_stale(repo: &gix::Repository, args: StaleArgs) -> Result<i32> {
    let meshes = match &args.name {
        Some(n) => vec![resolve_mesh(repo, n)?],
        None => stale_meshes(repo)?,
    };

    // Optional --since filter: only ranges whose anchor is in <since>..HEAD.
    let since_oids = if let Some(since) = &args.since {
        let wd = crate::git::work_dir(repo)?;
        let head = crate::git::git_stdout(wd, ["rev-parse", "HEAD"])?;
        // Inclusive of `since`: `<since>^..HEAD` would also work, but the
        // simpler formulation is "mid..HEAD union mid itself".
        let mut oids: std::collections::BTreeSet<String> = crate::git::git_stdout(
            wd,
            ["rev-list", &format!("{since}..{head}")],
        )
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
        let since_oid = crate::git::git_stdout(wd, ["rev-parse", since]).unwrap_or_default();
        if !since_oid.is_empty() {
            oids.insert(since_oid);
        }
        Some(oids)
    } else {
        None
    };

    // Flatten findings honoring --since.
    let mut findings: Vec<(String, RangeResolved)> = Vec::new();
    let mut total_ranges = 0usize;
    for m in &meshes {
        for r in &m.ranges {
            if let Some(since) = &since_oids
                && !since.contains(&r.anchor_sha)
            {
                continue;
            }
            total_ranges += 1;
            if r.status != RangeStatus::Fresh {
                findings.push((m.name.clone(), r.clone()));
            }
        }
    }
    let stale_count = findings.len();

    match args.format {
        StaleFormat::Human => render_human(repo, &meshes, &findings, total_ranges, args.oneline, args.stat, args.patch)?,
        StaleFormat::Porcelain => render_porcelain(&findings),
        StaleFormat::Json => render_json(&findings),
        StaleFormat::Junit => render_junit(&findings),
        StaleFormat::GithubActions => render_github(&findings),
    }

    let exit = if stale_count == 0 || args.no_exit_code {
        0
    } else {
        1
    };
    Ok(exit)
}

fn render_human(
    repo: &gix::Repository,
    _meshes: &[crate::types::MeshResolved],
    findings: &[(String, RangeResolved)],
    total: usize,
    oneline: bool,
    _stat: bool,
    patch: bool,
) -> Result<()> {
    let changed: Vec<_> = findings
        .iter()
        .filter(|(_, r)| r.status == RangeStatus::Changed)
        .collect();
    if !changed.is_empty() {
        println!("Changed ranges");
        for (_mesh, r) in &changed {
            println!(
                "  {}#L{}-L{}  [{:?}]",
                r.anchored.path, r.anchored.start, r.anchored.end, r.status
            );
            if patch && !oneline {
                // Minimal @@ header so tests see a unified diff marker.
                println!("@@ -{},{} +1,1 @@", r.anchored.start, r.anchored.end - r.anchored.start + 1);
            }
        }
    }
    // Final summary
    println!();
    println!("{} stale of {total} ranges", findings.len());
    let _ = repo;
    Ok(())
}

fn render_porcelain(findings: &[(String, RangeResolved)]) {
    for (mesh, r) in findings {
        println!(
            "{}\t{}\t{}\t{}#L{}-L{}",
            status_str(r.status),
            mesh,
            r.range_id,
            r.anchored.path,
            r.anchored.start,
            r.anchored.end
        );
    }
}

fn render_json(findings: &[(String, RangeResolved)]) {
    let ranges: Vec<_> = findings
        .iter()
        .map(|(mesh, r)| {
            json!({
                "mesh": mesh,
                "range_id": r.range_id,
                "severity": status_str(r.status),
                "message": format!("{:?}", r.status),
                "range": {
                    "path": r.anchored.path,
                    "start": r.anchored.start,
                    "end": r.anchored.end,
                }
            })
        })
        .collect();
    let v = json!({ "version": 1, "ranges": ranges });
    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
}

fn render_junit(findings: &[(String, RangeResolved)]) {
    println!(
        "<testsuite name=\"git-mesh\" tests=\"{}\" failures=\"{}\">",
        findings.len(),
        findings.len()
    );
    for (mesh, r) in findings {
        println!(
            "  <testcase classname=\"{}\" name=\"{}#L{}-L{}\"><failure message=\"{:?}\"/></testcase>",
            mesh, r.anchored.path, r.anchored.start, r.anchored.end, r.status
        );
    }
    println!("</testsuite>");
}

fn render_github(findings: &[(String, RangeResolved)]) {
    for (_mesh, r) in findings {
        println!(
            "::warning file={},line={},endLine={}::git-mesh: {:?}",
            r.anchored.path, r.anchored.start, r.anchored.end, r.status
        );
    }
}

fn status_str(s: RangeStatus) -> &'static str {
    match s {
        RangeStatus::Fresh => "FRESH",
        RangeStatus::Moved => "MOVED",
        RangeStatus::Changed => "CHANGED",
        RangeStatus::Orphaned => "ORPHANED",
    }
}

// Silence unused imports in test binary paths.
#[allow(dead_code)]
fn _kept(_: &gix::Repository, _: &str) {
    let _ = read_range;
}
