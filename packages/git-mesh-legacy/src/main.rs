mod cli;

use anyhow::{Context, Result, anyhow};
use cli::{
    commit::run_commit,
    show::{print_mesh, print_mesh_diff, print_mesh_format, print_mesh_list, print_mesh_log},
    stale_output::run_stale,
    structural::{run_mv, run_restore, run_rm},
    sync::{run_doctor, run_fetch, run_push},
    PrintOptions,
};
use clap::{Arg, ArgAction, ArgGroup, Command, value_parser};
use git_mesh_legacy::mesh_commit_info_at;
use git_mesh_legacy::read_mesh_at;

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
        Some(("stale", sub_matches)) => return run_stale(&repo, sub_matches),
        Some(("commit", sub_matches)) => run_commit(&repo, sub_matches)?,
        Some(("rm", sub_matches)) => run_rm(&repo, sub_matches)?,
        Some(("mv", sub_matches)) => run_mv(&repo, sub_matches)?,
        Some(("restore", sub_matches)) => run_restore(&repo, sub_matches)?,
        Some(("fetch", sub_matches)) => run_fetch(&repo, sub_matches)?,
        Some(("push", sub_matches)) => run_push(&repo, sub_matches)?,
        Some(("doctor", _)) => return run_doctor(&repo),
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
