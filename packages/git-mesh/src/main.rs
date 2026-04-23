use anyhow::{Context, Result, anyhow};
use clap::{Arg, ArgAction, Command, value_parser};
use git_mesh::{
    CommitInput, CopyDetection, LinkResolved, LinkStatus, Mesh, MeshCommitInfo, MeshResolved,
    RangeSpec, SideSpec, commit_mesh, list_mesh_names, mesh_commit_info, remove_mesh, rename_mesh,
    restore_mesh, show_mesh, stale_mesh,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let matches = cli().get_matches();
    let repo = gix::discover(".").context("not inside a git repository")?;

    match matches.subcommand() {
        Some(("stale", sub_matches)) => {
            let name = sub_matches.get_one::<String>("name").unwrap();
            let resolved = stale_mesh(&repo, name)?;
            print_stale(&resolved, &mesh_commit_info(&repo, name)?)?;
        }
        Some(("commit", sub_matches)) => {
            let name = sub_matches.get_one::<String>("name").unwrap();
            validate_mesh_name(name)?;

            let default_copy_detection = sub_matches
                .get_one::<String>("copy-detection")
                .map(|value| parse_copy_detection(value))
                .transpose()?;
            let default_ignore_whitespace =
                sub_matches.get_one::<bool>("ignore-whitespace").copied();

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

            let message = sub_matches.get_one::<String>("message").unwrap().clone();
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
        None => {
            if let Some(name) = matches.get_one::<String>("name") {
                let mesh = show_mesh(&repo, name)?;
                print_mesh(&mesh, &mesh_commit_info(&repo, name)?);
            } else {
                print_mesh_list(&repo)?;
            }
        }
        Some((command, _)) => return Err(anyhow!("unsupported subcommand `{command}`")),
    }

    Ok(())
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
        .subcommand(
            Command::new("stale")
                .about("Resolve and report drift for a mesh")
                .arg(Arg::new("name").required(true).value_name("NAME")),
        )
        .subcommand(
            Command::new("commit")
                .about("Create or update a mesh")
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
                        .required(true)
                        .value_name("MESSAGE"),
                )
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
                        .action(ArgAction::Set)
                        .default_missing_value("true")
                        .default_value("true")
                        .num_args(0..=1)
                        .value_parser(value_parser!(bool)),
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

fn print_mesh(mesh: &Mesh, info: &MeshCommitInfo) {
    println!("mesh {}", mesh.name);
    println!("commit {}", info.commit_oid);
    println!("Author: {} <{}>", info.author_name, info.author_email);
    println!("Date:   {}", info.author_date);
    println!();
    print_indented_message(&mesh.message);
    println!();
    println!("Links ({}):", mesh.links.len());
    for link in &mesh.links {
        println!("    {link}");
    }
}

fn print_stale(mesh: &MeshResolved, info: &MeshCommitInfo) -> Result<()> {
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
        }
    }

    Ok(())
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

fn parse_copy_detection(text: &str) -> Result<CopyDetection> {
    match text {
        "off" => Ok(CopyDetection::Off),
        "same-commit" => Ok(CopyDetection::SameCommit),
        "any-file-in-commit" => Ok(CopyDetection::AnyFileInCommit),
        "any-file-in-repo" => Ok(CopyDetection::AnyFileInRepo),
        _ => Err(anyhow!("invalid copy detection `{text}`")),
    }
}

fn validate_mesh_name(name: &str) -> Result<()> {
    const RESERVED: &[&str] = &[
        "commit", "rm", "mv", "restore", "stale", "fetch", "push", "doctor", "log", "help",
    ];
    anyhow::ensure!(!RESERVED.contains(&name), "mesh name `{name}` is reserved");
    Ok(())
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
    let anchored = format_range_spec(&RangeSpec {
        path: side.anchored.path.clone(),
        start: side.anchored.start,
        end: side.anchored.end,
    });

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

fn format_current_location(location: &git_mesh::LinkLocation) -> String {
    format!("{}#L{}-L{}", location.path, location.start, location.end)
}

fn format_range_spec(range: &RangeSpec) -> String {
    format!("{}#L{}-L{}", range.path, range.start, range.end)
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
