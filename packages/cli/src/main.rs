mod commands;
mod frontmatter;
mod git;
mod headings;
mod index;
mod parser;
mod perf;
mod version;

use std::io::{self, BufRead, IsTerminal};
use std::process;
use std::time::Instant;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use miette::{IntoDiagnostic, Result, WrapErr};

#[derive(Debug, Clone, ValueEnum)]
enum Format {
    Json,
}

/// Which git snapshot the wiki index reads from.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum SourceArg {
    Worktree,
    Index,
    Head,
}

#[derive(Debug, Parser)]
#[command(
    name = "wiki",
    version = crate::version::VERSION,
    before_help = concat!("wiki ", env!("WIKI_VERSION"), "\n"),
    about = "wiki - Read and maintain wiki pages",
    long_about = "wiki - Read and maintain wiki pages\n\nPass a query to search wiki pages with weighted ranking:\n  wiki [query]\n\nWith no arguments, wiki prints help and the wiki README when available.\n\nStdin is read when no argument is given for commands that accept it:\n  echo wiki/page.md | wiki summary\n\nCommand names (check, list, summary, mesh) are reserved and cannot be used as page titles.\n\nFile selection follows the current working directory; links, anchors, and mesh coverage resolve against the git repository root.",
    disable_help_subcommand = true,
    disable_version_flag = true,
)]
struct Cli {
    /// Output structured JSON instead of human-readable text.
    #[arg(long = "format", value_enum, global = true)]
    format: Option<Format>,

    /// Print the wiki CLI version.
    #[arg(short = 'v', long = "version", action = ArgAction::SetTrue, global = true)]
    version: bool,

    /// Emit per-event timings to stderr (also enabled by `WIKI_PERF=1`).
    #[arg(long = "perf", action = ArgAction::SetTrue, global = true)]
    perf: bool,

    /// Document source: working tree (default), git index, or HEAD commit.
    #[arg(
        long = "source",
        value_enum,
        default_value_t = SourceArg::Worktree,
        global = true
    )]
    source: SourceArg,

    /// Search query for the default wiki lookup.
    #[arg(value_name = "query")]
    query: Option<String>,

    /// Maximum number of search results to print.
    #[arg(
        short = 'l',
        long = "limit",
        value_parser = clap::value_parser!(i64).range(1..),
        default_value_t = index::SEARCH_LIMIT
    )]
    limit: i64,

    /// Skip the first N search results (for pagination).
    #[arg(
        short = 'o',
        long = "offset",
        value_parser = clap::value_parser!(usize),
        default_value_t = 0
    )]
    offset: usize,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Validate all links and frontmatter in wiki pages.
    ///
    /// Fragment links: referenced file exists, line ranges within bounds.
    /// Frontmatter: title required, aliases and tags valid, no title/alias
    /// collisions (case-insensitive).
    ///
    /// Always verifies that every fragment link with a line range is
    /// covered by a mesh that anchors both the wiki file and the link
    /// target (stored under `.wiki/`). With `--fix`, also creates that
    /// mesh coverage best-effort (the "Fix #4" pass).
    Check {
        /// Glob patterns to match wiki pages (default: `**/*.md` under the current directory)
        #[arg(value_name = "glob")]
        globs: Vec<String>,
        /// Exit 0 even when validation errors are found (report-only mode)
        #[arg(long = "no-exit-code")]
        no_exit_code: bool,
        /// Rewrite drifted links and anchors in place (requires --source=worktree).
        #[arg(long = "fix")]
        fix: bool,
        /// Print what would be rewritten without modifying any files (requires --fix).
        #[arg(long = "fix-dry-run", requires = "fix")]
        fix_dry_run: bool,
        /// Print only the repo-relative path of each created or extended mesh to
        /// stdout (one per line); route the fix/skip summary, advisories, and
        /// diagnostics to stderr. Lets callers stage exactly what this run touched.
        #[arg(
            long = "print-applied",
            requires = "fix",
            conflicts_with = "fix_dry_run",
            conflicts_with = "format"
        )]
        print_applied: bool,
    },

    /// List all wiki pages with metadata (title, aliases, tags, file path).
    ///
    /// Optionally filter by tag.
    List {
        /// Filter pages by tag
        #[arg(long = "tag", value_name = "tag")]
        tag: Option<String>,

        /// Return at most N entries from the title-ordered listing. Default: no limit.
        #[arg(long = "limit", value_name = "N", value_parser = clap::value_parser!(u64))]
        limit: Option<u64>,

        /// Skip the first N entries of the title-ordered listing. Default: 0.
        #[arg(long = "offset", value_name = "N", value_parser = clap::value_parser!(u64))]
        offset: Option<u64>,
    },

    /// Print the summary of a wiki page.
    ///
    /// Resolves the argument via title, alias, or file path (case-insensitive
    /// for title/alias), then writes the canonical title, absolute path, and
    /// summary to stdout. Reads from stdin when the argument is omitted. With
    /// --format json, emits { title, file, summary }.
    Summary {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "title|path")]
        title: Option<String>,
    },

    /// Inspect and manage `.wiki/` mesh anchors.
    ///
    /// Provides three verbs for in-process mesh reconciliation so that any
    /// `wiki check` failure can be resolved without the `git mesh` binary.
    ///
    ///   wiki mesh show <slug> [--patch]   — inspect anchors; diff on --patch
    ///   wiki mesh add  <slug> <anchor>... — upsert anchor(s) into a mesh
    ///   wiki mesh remove <slug> [anchor]  — drop an anchor or the whole mesh
    Mesh {
        #[command(subcommand)]
        command: MeshCommands,
    },
}

/// Subcommands for `wiki mesh`.
#[derive(Debug, Subcommand)]
enum MeshCommands {
    /// Show the anchors in a mesh, optionally with a before/after diff.
    ///
    /// Prints each anchor's path, line range, stored hash, and fresh/stale
    /// status (by recomputing the rk64 fingerprint against the worktree).
    /// With `--patch`, also shows the diff between the committed blob slice
    /// and the current worktree slice for each stale anchor.
    Show {
        /// The mesh slug (e.g. `auth/login-flow`)
        #[arg(value_name = "slug")]
        slug: String,
        /// Show a before/after diff for stale anchors (committed vs worktree)
        #[arg(long = "patch")]
        patch: bool,
    },
    /// Upsert an anchor into a mesh (create the mesh if it does not exist).
    ///
    /// `<anchor>` must be `path#Lstart-Lend` or bare `path` (whole file).
    /// When the mesh does not yet exist, `--why` is required. When the mesh
    /// already exists, `--why` is optional and overwrites the stored rationale
    /// (printing an explicit notice naming the previous rationale).
    ///
    /// At least one `<anchor>` OR `--why` must be given. The anchor-less form
    /// (`wiki mesh add <slug> --why "…"`) is a rationale-only update against an
    /// existing mesh.
    Add {
        /// The mesh slug (e.g. `auth/login-flow`)
        #[arg(value_name = "slug")]
        slug: String,
        /// Anchor(s): `path#Lstart-Lend` or bare `path` (whole file)
        #[arg(value_name = "anchor", required_unless_present = "why")]
        anchors: Vec<String>,
        /// Rationale text (required when creating a new mesh; updates the
        /// rationale when supplied for an existing mesh)
        #[arg(long = "why", value_name = "text")]
        why: Option<String>,
    },
    /// Remove an anchor from a mesh, or the whole mesh if no anchor is given.
    ///
    /// When an `<anchor>` argument is supplied, only that anchor is removed
    /// and the mesh file is deleted when the last anchor is dropped. When no
    /// `<anchor>` is supplied, the entire mesh file is deleted.
    Remove {
        /// The mesh slug (e.g. `auth/login-flow`)
        #[arg(value_name = "slug")]
        slug: String,
        /// Anchor to remove: `path#Lstart-Lend` or bare `path` (whole file).
        /// Omit to remove the entire mesh.
        #[arg(value_name = "anchor")]
        anchor: Option<String>,
    },
}

/// Read all non-empty trimmed lines from stdin, if stdin is not an interactive terminal.
///
/// Returns an empty vec when stdin is a tty or contains only whitespace.
fn read_stdin_lines() -> Vec<String> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return vec![];
    }
    stdin
        .lock()
        .lines()
        .map_while(Result::ok)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Resolve the CLI title arg or fall back to stdin lines.
///
/// Returns `Err` (exit 2) only when no input is available at all.
fn resolve_inputs(
    title: Option<String>,
    stdin: impl FnOnce() -> Vec<String>,
) -> Result<Vec<String>> {
    match title {
        Some(t) => Ok(vec![t]),
        None => {
            let lines = stdin();
            if lines.is_empty() {
                return Err(miette::miette!(
                    "no page title or path provided (pass as argument or via stdin)"
                ));
            }
            Ok(lines)
        }
    }
}

/// Run `f` for each input, returning the worst exit code seen.
fn run_for_each(
    inputs: Vec<String>,
    mut f: impl FnMut(&str) -> Result<i32>,
    separate: bool,
) -> Result<i32> {
    let mut exit = 0i32;
    for (i, input) in inputs.iter().enumerate() {
        if separate && i > 0 {
            println!("\n---");
        }
        let code = f(input)?;
        exit = exit.max(code);
    }
    Ok(exit)
}

fn main() {
    let cli = Cli::parse();
    if cli.version {
        println!("wiki {}", crate::version::VERSION);
        process::exit(0);
    }
    let json = matches!(cli.format, Some(Format::Json));
    perf::enable_stderr(cli.perf);

    if !json {
        miette::set_hook(Box::new(|_| {
            Box::new(miette::MietteHandlerOpts::new().build())
        }))
        .ok();
    }

    let source: index::DocSource = match cli.source {
        SourceArg::Worktree => index::DocSource::WorkingTree,
        SourceArg::Index => index::DocSource::Index,
        SourceArg::Head => index::DocSource::Head,
    };

    let result = run(cli.command, cli.query, cli.limit, cli.offset, json, source);

    match result {
        Ok(code) => process::exit(code),
        Err(e) => {
            if json {
                eprintln!("{}", serde_json::json!({ "error": e.to_string() }));
            } else {
                eprintln!("{e:?}");
            }
            process::exit(2);
        }
    }
}

fn run(
    command: Option<Commands>,
    query: Option<String>,
    limit: i64,
    offset: usize,
    json: bool,
    source: index::DocSource,
) -> Result<i32> {
    let repo_root = git::repo_root()?;
    let scan_root = std::env::current_dir()
        .into_diagnostic()
        .wrap_err("failed to read current working directory")?;

    let command_name = command_name(command.as_ref(), query.as_deref());
    perf::init(&repo_root, command_name, json);
    let _command_span = perf::span_for_command(command_name);
    let started = Instant::now();

    let result = match command {
        Some(Commands::Check {
            globs,
            no_exit_code,
            fix,
            fix_dry_run,
            print_applied,
        }) => {
            if fix && !matches!(source, index::DocSource::WorkingTree) {
                eprintln!("error: --fix requires --source=worktree");
                return Ok(2);
            }
            commands::check::run(
                &globs,
                json,
                &scan_root,
                &repo_root,
                no_exit_code,
                source,
                fix,
                fix_dry_run,
                print_applied,
            )
        }
        Some(Commands::List { tag, limit, offset }) => {
            commands::list::run(&[], tag.as_deref(), limit, offset, json, &repo_root, source)
        }
        Some(Commands::Summary { title }) => {
            let inputs = resolve_inputs(title, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::summary::run(input, json, &repo_root, source),
                false,
            )
        }
        Some(Commands::Mesh { command }) => commands::mesh::manage::run(command, &repo_root, json),
        None => match query.as_deref() {
            Some(query) => commands::search::run(query, limit, offset, json, &repo_root, source),
            None => {
                // No subcommand and no query: print help and the wiki README.
                let mut cmd = <Cli as clap::CommandFactory>::command();
                cmd.print_help().ok();
                println!();
                Ok(0)
            }
        },
    };

    match &result {
        Ok(exit_code) => perf::finish(
            command_name,
            *exit_code,
            started.elapsed().as_secs_f64() * 1000.0,
            "ok",
        ),
        Err(_) => perf::finish(
            command_name,
            2,
            started.elapsed().as_secs_f64() * 1000.0,
            "error",
        ),
    }

    result
}

fn command_name(command: Option<&Commands>, query: Option<&str>) -> &'static str {
    match command {
        Some(Commands::Check { .. }) => "check",
        Some(Commands::List { .. }) => "list",
        Some(Commands::Summary { .. }) => "summary",
        Some(Commands::Mesh { .. }) => "mesh",
        None if query.is_some() => "search",
        None => "help",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_query_and_limit() {
        let cli = Cli::try_parse_from(["wiki", "--limit", "7", "rust indexing"]).expect("parse");
        assert_eq!(cli.query.as_deref(), Some("rust indexing"));
        assert_eq!(cli.limit, 7);
        assert_eq!(cli.offset, 0);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_top_level_query_with_offset() {
        let cli = Cli::try_parse_from(["wiki", "--limit", "3", "--offset", "6", "runtime"])
            .expect("parse");
        assert_eq!(cli.query.as_deref(), Some("runtime"));
        assert_eq!(cli.limit, 3);
        assert_eq!(cli.offset, 6);
    }

    #[test]
    fn parses_short_offset_flag() {
        let cli = Cli::try_parse_from(["wiki", "-o", "3", "runtime"]).expect("parse");
        assert_eq!(cli.offset, 3);
    }

    #[test]
    fn reserved_subcommands_still_parse_as_subcommands() {
        let cli = Cli::try_parse_from(["wiki", "summary"]).expect("parse");
        assert!(matches!(cli.command, Some(Commands::Summary { .. })));
        assert!(cli.query.is_none());
    }
}
