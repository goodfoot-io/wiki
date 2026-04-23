use anyhow::Result;
use git_mesh::{fetch_mesh_refs, list_mesh_names, push_mesh_refs, read_link, read_mesh_at};

pub(crate) fn run_fetch(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let remote = fetch_mesh_refs(
        repo,
        sub_matches.get_one::<String>("remote").map(String::as_str),
    )?;
    println!("fetched mesh refs from {remote}");
    Ok(())
}

pub(crate) fn run_push(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let remote = push_mesh_refs(
        repo,
        sub_matches.get_one::<String>("remote").map(String::as_str),
    )?;
    println!("pushed mesh refs to {remote}");
    Ok(())
}

/// Per-mesh integrity report collected by `doctor`.
struct MeshReport {
    name: String,
    link_count: usize,
    issues: Vec<String>,
}

/// Run the integrity audit for `git mesh doctor`.
///
/// Output shape is two-section: a per-mesh status list, followed by a
/// summary footer with counts. Each mesh gets one of:
///
/// * `ok      <name>  (<n> links)` — mesh commit and every referenced
///   link blob read cleanly.
/// * `ISSUE   <name>` followed by indented bullets for every concrete
///   integrity problem (unreadable mesh, unreadable/missing link blob).
///
/// Exit code discipline per docs/git-mesh.md §10.4:
///
/// * `0` — no integrity problems.
/// * `2` — at least one integrity problem. This is a **tool-level** error
///   ("the stored data is broken"), distinct from `stale --exit-code`'s
///   `1` ("stale findings"). CI can tell the two apart.
pub(crate) fn run_doctor(repo: &gix::Repository) -> Result<i32> {
    let (reports, enumeration_failure) = collect_doctor_reports(repo);

    let mesh_count = reports.len();
    let ok_count = reports.iter().filter(|r| r.issues.is_empty()).count();
    let issue_mesh_count = mesh_count - ok_count;
    let issue_total: usize = reports.iter().map(|r| r.issues.len()).sum();
    let total_issues =
        issue_total + if enumeration_failure.is_some() { 1 } else { 0 };

    println!("mesh doctor: checking refs/meshes/v1/*");

    if let Some(message) = &enumeration_failure {
        println!("  ISSUE   <ref enumeration>");
        println!("            - {message}");
    }

    for report in &reports {
        if report.issues.is_empty() {
            let plural = if report.link_count == 1 { "" } else { "s" };
            println!(
                "  ok      {}  ({} link{plural})",
                report.name, report.link_count
            );
        } else {
            println!("  ISSUE   {}", report.name);
            for issue in &report.issues {
                println!("            - {issue}");
            }
        }
    }

    println!();
    if total_issues == 0 {
        if mesh_count == 0 {
            println!("mesh doctor: ok (no meshes)");
        } else {
            let plural = if mesh_count == 1 { "" } else { "es" };
            println!("mesh doctor: ok ({mesh_count} mesh{plural} checked)");
        }
        Ok(0)
    } else {
        let issue_plural = if total_issues == 1 { "" } else { "s" };
        if enumeration_failure.is_some() {
            println!(
                "mesh doctor: found {total_issues} issue{issue_plural} (ref enumeration failed)"
            );
        } else {
            let mesh_plural = if issue_mesh_count == 1 { "" } else { "es" };
            println!(
                "mesh doctor: found {total_issues} issue{issue_plural} across {issue_mesh_count} mesh{mesh_plural} ({ok_count}/{mesh_count} ok)"
            );
        }
        // Exit 2 = tool-level error (broken data). Distinct from `stale
        // --exit-code`'s exit 1 (stale findings). See §10.4.
        Ok(2)
    }
}

fn collect_doctor_reports(
    repo: &gix::Repository,
) -> (Vec<MeshReport>, Option<String>) {
    let names = match list_mesh_names(repo) {
        Ok(names) => names,
        Err(error) => {
            return (
                Vec::new(),
                Some(format!("failed to list mesh refs: {error:#}")),
            );
        }
    };

    let mut reports = Vec::with_capacity(names.len());
    for name in names {
        let mut issues = Vec::new();
        let mut link_count = 0usize;
        match read_mesh_at(repo, &name, None) {
            Ok(mesh) => {
                link_count = mesh.links.len();
                for link in mesh.links {
                    if let Err(error) = read_link(repo, &link.id) {
                        issues.push(format!(
                            "link `{}` is unreadable: {error:#}",
                            link.id
                        ));
                    }
                }
            }
            Err(error) => {
                issues.push(format!("mesh is unreadable: {error:#}"));
            }
        }
        reports.push(MeshReport {
            name,
            link_count,
            issues,
        });
    }

    // Sort: issue meshes first (worst-first grouping), then by name.
    reports.sort_by(|a, b| {
        let a_bad = !a.issues.is_empty();
        let b_bad = !b.issues.is_empty();
        b_bad.cmp(&a_bad).then_with(|| a.name.cmp(&b.name))
    });

    (reports, None)
}
