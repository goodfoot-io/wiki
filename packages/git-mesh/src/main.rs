use anyhow::{Context, Result, anyhow};
use clap::{Arg, ArgAction, ArgGroup, Command, value_parser};
use git_mesh::{
    CommitInput, CopyDetection, CulpritCommit, LinkResolved, LinkStatus, MeshCommitInfo,
    MeshResolved, MeshStored, RangeSpec, SideSpec, StoredLink, commit_mesh, fetch_mesh_refs,
    is_ancestor_commit, list_mesh_names, mesh_commit_info, mesh_commit_info_at, mesh_log,
    push_mesh_refs, read_git_text, read_link, read_mesh_at, remove_mesh, rename_mesh,
    resolve_commit_ish, restore_mesh, show_mesh, stale_mesh, validate_mesh_name,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::process::Command as ProcessCommand;

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32> {
    let matches = cli().get_matches();
    let repo = gix::discover(".").context("not inside a git repository")?;

    match matches.subcommand() {
        Some(("stale", sub_matches)) => {
            let detail = parse_stale_detail(sub_matches)?;
            let format = parse_stale_format(
                sub_matches
                    .get_one::<String>("format")
                    .map(String::as_str)
                    .unwrap_or("human"),
            )?;
            let since = sub_matches
                .get_one::<String>("since")
                .map(|value| resolve_commit_ish(&repo, value))
                .transpose()?;
            let reports = load_stale_reports(
                &repo,
                sub_matches.get_one::<String>("name"),
                since.as_deref(),
            )?;

            match format {
                StaleFormat::Human => print_human_stale(&repo, &reports, detail)?,
                StaleFormat::Porcelain => print_porcelain_stale(&reports),
                StaleFormat::Json => print_json_stale(&reports)?,
                StaleFormat::Junit => print_junit_stale(&reports)?,
                StaleFormat::GitHubActions => print_github_actions_stale(&reports),
            }

            if sub_matches.get_flag("exit-code") && reports_have_stale(&reports) {
                return Ok(1);
            }
        }
        Some(("commit", sub_matches)) => {
            let name = sub_matches.get_one::<String>("name").unwrap();
            validate_mesh_name(name)?;

            let default_copy_detection = sub_matches
                .get_one::<String>("copy-detection")
                .map(|value| parse_copy_detection(value))
                .transpose()?;
            let default_ignore_whitespace = if sub_matches.get_flag("no-ignore-whitespace") {
                Some(false)
            } else {
                Some(true)
            };

            let adds = sub_matches
                .get_many::<String>("link")
                .into_iter()
                .flatten()
                .map(|value| {
                    parse_link_pair(value, default_copy_detection, default_ignore_whitespace)
                })
                .collect::<Result<Vec<_>>>()?;
            let removes = sub_matches
                .get_many::<String>("unlink")
                .into_iter()
                .flatten()
                .map(|value| parse_range_pair(value))
                .collect::<Result<Vec<_>>>()?;

            let message = resolve_commit_message(&repo, name, sub_matches)?;
            let anchor_sha = sub_matches.get_one::<String>("anchor").cloned();
            let amend = sub_matches.get_flag("amend");

            commit_mesh(
                &repo,
                CommitInput {
                    name: name.clone(),
                    adds,
                    removes,
                    message,
                    anchor_sha,
                    expected_tip: None,
                    amend,
                },
            )?;

            let ref_name = format!("refs/meshes/v1/{name}");
            println!("updated {ref_name}");
        }
        Some(("rm", sub_matches)) => {
            let name = sub_matches.get_one::<String>("name").unwrap();
            remove_mesh(&repo, name)?;
            println!("deleted refs/meshes/v1/{name}");
        }
        Some(("mv", sub_matches)) => {
            let old_name = sub_matches.get_one::<String>("old").unwrap();
            let new_name = sub_matches.get_one::<String>("new").unwrap();
            validate_mesh_name(new_name)?;
            rename_mesh(&repo, old_name, new_name, sub_matches.get_flag("keep"))?;
            if sub_matches.get_flag("keep") {
                println!("copied refs/meshes/v1/{old_name} to refs/meshes/v1/{new_name}");
            } else {
                println!("renamed refs/meshes/v1/{old_name} to refs/meshes/v1/{new_name}");
            }
        }
        Some(("restore", sub_matches)) => {
            let name = sub_matches.get_one::<String>("name").unwrap();
            let commit_ish = sub_matches.get_one::<String>("commit-ish").unwrap();
            restore_mesh(&repo, name, commit_ish)?;
            println!("restored refs/meshes/v1/{name} from {commit_ish}");
        }
        Some(("fetch", sub_matches)) => {
            let remote = fetch_mesh_refs(
                &repo,
                sub_matches.get_one::<String>("remote").map(String::as_str),
            )?;
            println!("fetched mesh refs from {remote}");
        }
        Some(("push", sub_matches)) => {
            let remote = push_mesh_refs(
                &repo,
                sub_matches.get_one::<String>("remote").map(String::as_str),
            )?;
            println!("pushed mesh refs to {remote}");
        }
        Some(("doctor", _)) => {
            let issues = run_doctor(&repo);
            if issues.is_empty() {
                println!("mesh doctor: ok");
            } else {
                println!("mesh doctor: found {} issue(s)", issues.len());
                for issue in issues {
                    println!("{issue}");
                }
                return Ok(1);
            }
        }
        None => {
            if let Some(name) = matches.get_one::<String>("name") {
                if let Some(revision_range) = matches.get_one::<String>("diff") {
                    print_mesh_diff(&repo, name, revision_range)?;
                } else if matches.get_flag("log") {
                    let limit = matches.get_one::<usize>("limit").copied();
                    print_mesh_log(&repo, name, matches.get_flag("oneline"), limit)?;
                } else {
                    let at = matches.get_one::<String>("at").map(String::as_str);
                    let mesh = read_mesh_at(&repo, name, at)?;
                    let info = mesh_commit_info_at(&repo, name, at)?;
                    let options = PrintOptions {
                        oneline: matches.get_flag("oneline"),
                        no_abbrev: matches.get_flag("no-abbrev"),
                    };
                    if let Some(format) = matches.get_one::<String>("format") {
                        print_mesh_format(&mesh, &info, format, options)?;
                    } else {
                        print_mesh(&mesh, &info, options);
                    }
                }
            } else {
                print_mesh_list(&repo)?;
            }
        }
        Some((command, _)) => return Err(anyhow!("unsupported subcommand `{command}`")),
    }

    Ok(0)
}

fn cli() -> Command {
    Command::new("git-mesh")
        .about("Track relationships between anchored code ranges")
        .subcommand_required(false)
        .args_conflicts_with_subcommands(true)
        .arg(
            Arg::new("name")
                .value_name("NAME")
                .help("Show the named mesh"),
        )
        .arg(Arg::new("at").long("at").value_name("COMMIT_ISH"))
        .arg(Arg::new("log").long("log").action(ArgAction::SetTrue))
        .arg(Arg::new("diff").long("diff").value_name("REV..REV"))
        .arg(
            Arg::new("oneline")
                .long("oneline")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_name("FMT")
                .requires("name")
                .conflicts_with_all(["oneline", "log", "diff"]),
        )
        .arg(
            Arg::new("no-abbrev")
                .long("no-abbrev")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_name("N")
                .value_parser(value_parser!(usize)),
        )
        .subcommand(
            Command::new("stale")
                .about("Resolve and report drift for a mesh")
                .arg(Arg::new("name").required(false).value_name("NAME"))
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_name("FMT")
                        .default_value("human")
                        .value_parser(["human", "porcelain", "json", "junit", "github-actions"]),
                )
                .arg(
                    Arg::new("exit-code")
                        .long("exit-code")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("oneline")
                        .long("oneline")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["stat", "patch"]),
                )
                .arg(
                    Arg::new("stat")
                        .long("stat")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["oneline", "patch"]),
                )
                .arg(
                    Arg::new("patch")
                        .long("patch")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["oneline", "stat"]),
                )
                .arg(Arg::new("since").long("since").value_name("COMMIT_ISH")),
        )
        .subcommand(
            Command::new("commit")
                .about("Create or update a mesh")
                .group(
                    ArgGroup::new("message-source")
                        .args(["message", "message-file", "edit"])
                        .required(true),
                )
                .arg(Arg::new("name").required(true).value_name("NAME"))
                .arg(
                    Arg::new("link")
                        .long("link")
                        .value_name("RANGE_A:RANGE_B")
                        .action(ArgAction::Append),
                )
                .arg(
                    Arg::new("unlink")
                        .long("unlink")
                        .value_name("RANGE_A:RANGE_B")
                        .action(ArgAction::Append),
                )
                .arg(
                    Arg::new("message")
                        .short('m')
                        .long("message")
                        .value_name("MESSAGE"),
                )
                .arg(Arg::new("message-file").short('F').value_name("FILE"))
                .arg(Arg::new("edit").long("edit").action(ArgAction::SetTrue))
                .arg(Arg::new("amend").long("amend").action(ArgAction::SetTrue))
                .arg(
                    Arg::new("anchor")
                        .long("anchor")
                        .visible_alias("at")
                        .value_name("COMMIT_ISH"),
                )
                .arg(
                    Arg::new("copy-detection")
                        .long("copy-detection")
                        .value_parser([
                            "off",
                            "same-commit",
                            "any-file-in-commit",
                            "any-file-in-repo",
                        ])
                        .value_name("MODE"),
                )
                .arg(
                    Arg::new("ignore-whitespace")
                        .long("ignore-whitespace")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("no-ignore-whitespace"),
                )
                .arg(
                    Arg::new("no-ignore-whitespace")
                        .long("no-ignore-whitespace")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("rm")
                .about("Delete a mesh")
                .arg(Arg::new("name").required(true).value_name("NAME")),
        )
        .subcommand(
            Command::new("mv")
                .about("Rename or copy a mesh")
                .arg(Arg::new("keep").long("keep").action(ArgAction::SetTrue))
                .arg(Arg::new("old").required(true).value_name("OLD"))
                .arg(Arg::new("new").required(true).value_name("NEW")),
        )
        .subcommand(
            Command::new("restore")
                .about("Restore a mesh to an earlier state")
                .arg(Arg::new("name").required(true).value_name("NAME"))
                .arg(
                    Arg::new("commit-ish")
                        .required(true)
                        .value_name("COMMIT_ISH"),
                ),
        )
        .subcommand(
            Command::new("fetch")
                .about("Fetch mesh refs from a remote")
                .arg(Arg::new("remote").required(false).value_name("REMOTE")),
        )
        .subcommand(
            Command::new("push")
                .about("Push mesh refs to a remote")
                .arg(Arg::new("remote").required(false).value_name("REMOTE")),
        )
        .subcommand(Command::new("doctor").about("Validate mesh refs and link objects"))
}

fn print_mesh_list(repo: &gix::Repository) -> Result<()> {
    let names = list_mesh_names(repo)?;
    if names.is_empty() {
        println!("no meshes");
        return Ok(());
    }

    for name in names {
        let mesh = show_mesh(repo, &name)?;
        let summary = mesh.message.lines().next().unwrap_or_default();
        println!("{name}\t{} links\t{summary}", mesh.links.len());
    }

    Ok(())
}

#[derive(Clone, Copy)]
struct PrintOptions {
    oneline: bool,
    no_abbrev: bool,
}

fn print_mesh(mesh: &MeshStored, info: &MeshCommitInfo, options: PrintOptions) {
    if options.oneline {
        for link in stored_links_sorted(&mesh.links) {
            println!(
                "{}  {}",
                maybe_abbreviate(&link.anchor_sha, options.no_abbrev),
                format_link_pair(link)
            );
        }
        return;
    }

    println!("mesh {}", mesh.name);
    println!("commit {}", info.commit_oid);
    println!("Author: {} <{}>", info.author_name, info.author_email);
    println!("Date:   {}", info.author_date);
    println!();
    print_indented_message(&mesh.message);
    println!();
    println!("Links ({}):", mesh.links.len());
    for link in stored_links_sorted(&mesh.links) {
        println!(
            "    {}  {}",
            maybe_abbreviate(&link.anchor_sha, options.no_abbrev),
            format_link_pair(link)
        );
    }
}

fn print_mesh_format(
    mesh: &MeshStored,
    info: &MeshCommitInfo,
    format: &str,
    options: PrintOptions,
) -> Result<()> {
    let mut output = String::new();
    let mut chars = format.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }

        match chars.next() {
            Some('%') => output.push('%'),
            Some('n') => output.push('\n'),
            Some('m') => output.push_str(&mesh.name),
            Some('H') => output.push_str(&info.commit_oid),
            Some('h') => output.push_str(maybe_abbreviate(&info.commit_oid, options.no_abbrev)),
            Some('s') => output.push_str(&info.summary),
            Some('B') => output.push_str(&mesh.message),
            Some('a') => match chars.next() {
                Some('n') => output.push_str(&info.author_name),
                Some('e') => output.push_str(&info.author_email),
                Some('d') => output.push_str(&info.author_date),
                Some(other) => return Err(anyhow!("unsupported format token `%a{other}`")),
                None => return Err(anyhow!("dangling format token `%a`")),
            },
            Some('L') => output.push_str(&format_links_block(mesh, options.no_abbrev)),
            Some('l') => output.push_str(&mesh.links.len().to_string()),
            Some(other) => return Err(anyhow!("unsupported format token `%{other}`")),
            None => return Err(anyhow!("dangling trailing `%` in format string")),
        }
    }

    print!("{output}");
    if !output.ends_with('\n') {
        println!();
    }
    Ok(())
}

fn print_mesh_log(
    repo: &gix::Repository,
    name: &str,
    oneline: bool,
    limit: Option<usize>,
) -> Result<()> {
    let entries = mesh_log(repo, name, limit)?;
    for info in entries {
        if oneline {
            println!("{} {}", abbreviate_oid(&info.commit_oid), info.summary);
        } else {
            println!("commit {}", info.commit_oid);
            println!("Author: {} <{}>", info.author_name, info.author_email);
            println!("Date:   {}", info.author_date);
            println!();
            print_indented_message(&info.summary);
            println!();
        }
    }
    Ok(())
}

fn print_mesh_diff(repo: &gix::Repository, name: &str, revision_range: &str) -> Result<()> {
    let (left_rev, right_rev) = revision_range
        .split_once("..")
        .ok_or_else(|| anyhow!("invalid diff range `{revision_range}`; expected <rev>..<rev>"))?;
    anyhow::ensure!(
        !left_rev.is_empty() && !right_rev.is_empty(),
        "invalid diff range `{revision_range}`; expected <rev>..<rev>"
    );

    let left = read_mesh_at(repo, name, Some(left_rev))?;
    let right = read_mesh_at(repo, name, Some(right_rev))?;
    let left_info = mesh_commit_info_at(repo, name, Some(left_rev))?;
    let right_info = mesh_commit_info_at(repo, name, Some(right_rev))?;

    let left_links = index_links_by_pair(&left.links);
    let right_links = index_links_by_pair(&right.links);
    let all_pairs: BTreeSet<_> = left_links
        .keys()
        .cloned()
        .chain(right_links.keys().cloned())
        .collect();

    println!("mesh {}", name);
    println!("diff {}..{}", left_info.commit_oid, right_info.commit_oid);
    println!();

    for pair in all_pairs {
        match (left_links.get(&pair), right_links.get(&pair)) {
            (None, Some(link)) => {
                println!("+ {} @ {}", pair, abbreviate_oid(&link.anchor_sha));
            }
            (Some(link), None) => {
                println!("- {} @ {}", pair, abbreviate_oid(&link.anchor_sha));
            }
            (Some(left_link), Some(right_link))
                if left_link.anchor_sha != right_link.anchor_sha =>
            {
                println!(
                    "~ {} @ {} -> {}",
                    pair,
                    abbreviate_oid(&left_link.anchor_sha),
                    abbreviate_oid(&right_link.anchor_sha)
                );
            }
            _ => {}
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaleFormat {
    Human,
    Porcelain,
    Json,
    Junit,
    GitHubActions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaleDetail {
    Full,
    Oneline,
    Stat,
    Patch,
}

#[derive(Clone, Debug)]
struct StaleMeshReport {
    mesh: MeshResolved,
    info: MeshCommitInfo,
}

fn parse_stale_format(value: &str) -> Result<StaleFormat> {
    match value {
        "human" => Ok(StaleFormat::Human),
        "porcelain" => Ok(StaleFormat::Porcelain),
        "json" => Ok(StaleFormat::Json),
        "junit" => Ok(StaleFormat::Junit),
        "github-actions" => Ok(StaleFormat::GitHubActions),
        _ => Err(anyhow!("invalid stale format `{value}`")),
    }
}

fn parse_stale_detail(matches: &clap::ArgMatches) -> Result<StaleDetail> {
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

fn print_human_stale(
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
                for (index, side) in link.sides.iter().enumerate() {
                    let branch = if index == 0 { "├─" } else { "└─" };
                    println!(
                        "             {branch} {:<10} {}",
                        format_status(side.status),
                        format_side_summary(side)
                    );
                    if let Some(culprit) = &side.culprit {
                        println!(
                            "                caused by {} {}",
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
                    println!("               {}", link.reconcile_command);
                }
            }
        }
    }

    Ok(())
}

fn print_porcelain_stale(reports: &[StaleMeshReport]) {
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

fn print_json_stale(reports: &[StaleMeshReport]) -> Result<()> {
    #[derive(Serialize)]
    struct JsonSide {
        status: LinkStatus,
        anchored: String,
        current: Option<String>,
        culprit: Option<CulpritCommit>,
    }

    #[derive(Serialize)]
    struct JsonLink {
        id: String,
        status: LinkStatus,
        anchor_sha: String,
        pair: String,
        current_pair: String,
        reconcile_command: String,
        sides: [JsonSide; 2],
    }

    #[derive(Serialize)]
    struct JsonMesh<'a> {
        name: &'a str,
        commit_oid: &'a str,
        stale_count: usize,
        link_count: usize,
        links: Vec<JsonLink>,
    }

    #[derive(Serialize)]
    struct JsonReport<'a> {
        version: u32,
        meshes: Vec<JsonMesh<'a>>,
    }

    let payload = JsonReport {
        version: 1,
        meshes: reports
            .iter()
            .map(|report| JsonMesh {
                name: &report.mesh.name,
                commit_oid: &report.info.commit_oid,
                stale_count: report
                    .mesh
                    .links
                    .iter()
                    .filter(|link| link.status != LinkStatus::Fresh)
                    .count(),
                link_count: report.mesh.links.len(),
                links: sorted_links(&report.mesh)
                    .into_iter()
                    .map(|link| JsonLink {
                        id: link.link_id.clone(),
                        status: link.status,
                        anchor_sha: link.anchor_sha.clone(),
                        pair: format_resolved_pair(&link).expect("format pair"),
                        current_pair: format_current_pair(&link),
                        reconcile_command: link.reconcile_command.clone(),
                        sides: [
                            JsonSide {
                                status: link.sides[0].status,
                                anchored: format_side_anchored(&link.sides[0]),
                                current: link.sides[0]
                                    .current
                                    .as_ref()
                                    .map(format_current_location),
                                culprit: link.sides[0].culprit.clone(),
                            },
                            JsonSide {
                                status: link.sides[1].status,
                                anchored: format_side_anchored(&link.sides[1]),
                                current: link.sides[1]
                                    .current
                                    .as_ref()
                                    .map(format_current_location),
                                culprit: link.sides[1].culprit.clone(),
                            },
                        ],
                    })
                    .collect(),
            })
            .collect(),
    };
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn print_junit_stale(reports: &[StaleMeshReport]) -> Result<()> {
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

fn print_github_actions_stale(reports: &[StaleMeshReport]) {
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

fn print_indented_message(message: &str) {
    for line in message.lines() {
        println!("    {line}");
    }
}

fn parse_link_pair(
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

fn parse_range_pair(text: &str) -> Result<[RangeSpec; 2]> {
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

fn parse_range(text: &str) -> Result<RangeSpec> {
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

fn resolve_commit_message(
    repo: &gix::Repository,
    name: &str,
    matches: &clap::ArgMatches,
) -> Result<String> {
    if let Some(message) = matches.get_one::<String>("message") {
        return Ok(message.clone());
    }

    if let Some(path) = matches.get_one::<String>("message-file") {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read message file `{path}`"));
    }

    if matches.get_flag("edit") {
        let template = if matches.get_flag("amend") {
            read_mesh_at(repo, name, None)
                .map(|mesh| mesh.message)
                .unwrap_or_default()
        } else {
            String::new()
        };
        return edit_commit_message(repo, &template);
    }

    Err(anyhow!("missing commit message"))
}

fn edit_commit_message(repo: &gix::Repository, initial: &str) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let path = std::env::temp_dir().join(format!("git-mesh-msg-{}.txt", uuid::Uuid::new_v4()));
    fs::write(&path, initial)?;

    let editor = std::env::var("GIT_EDITOR")
        .or_else(|_| std::env::var("EDITOR"))
        .or_else(|_| git_editor(work_dir))
        .unwrap_or_else(|_| "vi".to_string());

    let status = ProcessCommand::new("sh")
        .current_dir(work_dir)
        .arg("-lc")
        .arg("\"$1\" \"$2\"")
        .arg("git-mesh-editor")
        .arg(editor)
        .arg(
            path.to_str()
                .ok_or_else(|| anyhow!("message file path is not valid UTF-8"))?,
        )
        .status()?;
    anyhow::ensure!(status.success(), "editor exited with status {status}");

    let message = fs::read_to_string(&path)?;
    let _ = fs::remove_file(&path);
    anyhow::ensure!(
        !message.trim().is_empty(),
        "aborting commit due to empty commit message"
    );
    Ok(message)
}

fn git_editor(work_dir: &std::path::Path) -> Result<String> {
    let output = ProcessCommand::new("git")
        .current_dir(work_dir)
        .args(["var", "GIT_EDITOR"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git var GIT_EDITOR failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn parse_copy_detection(text: &str) -> Result<CopyDetection> {
    match text {
        "off" => Ok(CopyDetection::Off),
        "same-commit" => Ok(CopyDetection::SameCommit),
        "any-file-in-commit" => Ok(CopyDetection::AnyFileInCommit),
        "any-file-in-repo" => Ok(CopyDetection::AnyFileInRepo),
        _ => Err(anyhow!("invalid copy detection `{text}`")),
    }
}

fn format_resolved_pair(link: &LinkResolved) -> Result<String> {
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

fn format_side_summary(side: &git_mesh::SideResolved) -> String {
    let anchored = format_side_anchored(side);

    match &side.current {
        Some(current)
            if current.path != side.anchored.path
                || current.start != side.anchored.start
                || current.end != side.anchored.end =>
        {
            format!("{anchored} -> {}", format_current_location(current))
        }
        Some(_) | None => anchored,
    }
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

fn format_current_location(location: &git_mesh::LinkLocation) -> String {
    format!("{}#L{}-L{}", location.path, location.start, location.end)
}

fn format_side_anchored(side: &git_mesh::SideResolved) -> String {
    format_range_spec(&RangeSpec {
        path: side.anchored.path.clone(),
        start: side.anchored.start,
        end: side.anchored.end,
    })
}

fn format_current_pair(link: &LinkResolved) -> String {
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

fn format_range_spec(range: &RangeSpec) -> String {
    format!("{}#L{}-L{}", range.path, range.start, range.end)
}

fn format_stored_side(side: &git_mesh::LinkSide) -> String {
    format_range_spec(&RangeSpec {
        path: side.path.clone(),
        start: side.start,
        end: side.end,
    })
}

fn format_link_pair(link: &StoredLink) -> String {
    format!(
        "{}:{}",
        format_stored_side(&link.sides[0]),
        format_stored_side(&link.sides[1])
    )
}

fn format_links_block(mesh: &MeshStored, no_abbrev: bool) -> String {
    stored_links_sorted(&mesh.links)
        .into_iter()
        .map(|link| {
            format!(
                "{}  {}",
                maybe_abbreviate(&link.anchor_sha, no_abbrev),
                format_link_pair(link)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn stored_links_sorted(links: &[StoredLink]) -> Vec<&StoredLink> {
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

fn index_links_by_pair(links: &[StoredLink]) -> BTreeMap<String, &StoredLink> {
    links
        .iter()
        .map(|link| (format_link_pair(link), link))
        .collect()
}

fn maybe_abbreviate(oid: &str, no_abbrev: bool) -> &str {
    if no_abbrev { oid } else { abbreviate_oid(oid) }
}

fn abbreviate_oid(oid: &str) -> &str {
    let end = oid.len().min(8);
    &oid[..end]
}

fn format_status(status: LinkStatus) -> &'static str {
    match status {
        LinkStatus::Fresh => "FRESH",
        LinkStatus::Moved => "MOVED",
        LinkStatus::Modified => "MODIFIED",
        LinkStatus::Rewritten => "REWRITTEN",
        LinkStatus::Missing => "MISSING",
        LinkStatus::Orphaned => "ORPHANED",
    }
}

fn status_rank(status: LinkStatus) -> u8 {
    match status {
        LinkStatus::Orphaned => 5,
        LinkStatus::Missing => 4,
        LinkStatus::Rewritten => 3,
        LinkStatus::Modified => 2,
        LinkStatus::Moved => 1,
        LinkStatus::Fresh => 0,
    }
}

fn highest_status(mesh: &MeshResolved) -> u8 {
    mesh.links
        .iter()
        .map(|link| status_rank(link.status))
        .max()
        .unwrap_or(0)
}

fn sorted_links(mesh: &MeshResolved) -> Vec<LinkResolved> {
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
    side: &git_mesh::SideResolved,
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

fn run_doctor(repo: &gix::Repository) -> Vec<String> {
    let mut issues = Vec::new();
    let Ok(names) = list_mesh_names(repo) else {
        issues.push("failed to list mesh refs".to_string());
        return issues;
    };

    for name in names {
        match read_mesh_at(repo, &name, None) {
            Ok(mesh) => {
                for link in mesh.links {
                    if let Err(error) = read_link(repo, &link.id) {
                        issues.push(format!(
                            "mesh `{name}` link `{}` is unreadable: {error:#}",
                            link.id
                        ));
                    }
                }
            }
            Err(error) => issues.push(format!("mesh `{name}` is unreadable: {error:#}")),
        }
    }

    issues
}
