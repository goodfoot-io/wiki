mod commands;
// Phase 0 tdd-bootstrap stubs — first consumers land in Phases 1–2, which
// removes the allow.
#[allow(dead_code)]
mod cache;
mod frontmatter;
mod git;
mod headings;
mod index;
mod parser;
mod perf;
mod store;
// Phase 0 tdd-bootstrap stubs — first consumers land in Phases 1–2, which
// removes the allow.
#[allow(dead_code)]
mod rk64;
mod version;
mod wikiignore;

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
    long_about = "wiki - Read and maintain wiki pages\n\nPass a query to search wiki pages with weighted ranking:\n  wiki [query]\n\nWith no arguments, wiki prints help and the wiki README when available.\n\nStdin is read when no argument is given for commands that accept it:\n  echo wiki/page.md | wiki summary\n\nCommand names (check, list, summary) are reserved and cannot be used as page titles.\n\nFile selection follows the current working directory; links and anchors resolve against the git repository root.",
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
    /// Line-range links are classified against the page's git-derived
    /// anchor epoch (the `links-reviewed:` frontmatter field): healthy,
    /// uncertified, broken, drifted, moved, or unverifiable. With `--fix`,
    /// relocated links are rewritten to follow their content, broken
    /// targets route through the rename machinery, and field-less pages
    /// carrying line-range links get the field initialized.
    ///
    /// Files matched by `./.wikiignore` (gitignore-syntax, one
    /// pattern per line, matched relative to the repo root) are
    /// excluded from discovery entirely, before frontmatter or link
    /// validation ever runs. Use it to keep non-wiki Markdown (e.g.
    /// `CLAUDE.md`) out of `wiki check`.
    Check {
        /// Glob patterns to match wiki pages (default: `**/*.md` under the current directory)
        #[arg(value_name = "glob")]
        globs: Vec<String>,
        /// Exit 0 even when validation errors are found (report-only mode)
        #[arg(long = "no-exit-code")]
        no_exit_code: bool,
        /// Rewrite drifted links in place (requires --source=worktree).
        #[arg(long = "fix")]
        fix: bool,
        /// Print what would be rewritten without modifying any files (requires --fix).
        /// Pending crash-recovery journals are accounted for as if already replayed,
        /// so the proposal matches what a real run will do.
        #[arg(long = "fix-dry-run", requires = "fix")]
        fix_dry_run: bool,
        /// Print only the repo-relative path of each file the run rewrote to
        /// stdout (one per line); route the fix/skip summary, advisories, and
        /// diagnostics to stderr. Lets callers stage exactly what this run touched
        /// — including files completed by crash-recovery journal replay.
        #[arg(
            long = "print-applied",
            requires = "fix",
            conflicts_with = "fix_dry_run",
            conflicts_with = "format"
        )]
        print_applied: bool,
        /// Best-effort delete of the anchor cache directory under the
        /// repository's common git dir (plan decision 8): prints the deleted
        /// path and exits 0 whether or not it existed.
        #[arg(long = "clear-cache")]
        clear_cache: bool,
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
    // Capture process entry as early as possible so the `startup` perf event
    // measures the pre-command-span residual (process spawn + repo-root
    // resolution) that the command span itself cannot see.
    let process_start = Instant::now();
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

    let result = run(
        cli.command,
        cli.query,
        cli.limit,
        cli.offset,
        json,
        source,
        process_start,
    );

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
    process_start: Instant,
) -> Result<i32> {
    let repo_root = git::repo_root()?;
    let scan_root = std::env::current_dir()
        .into_diagnostic()
        .wrap_err("failed to read current working directory")?;

    let command_name = command_name(command.as_ref(), query.as_deref());
    perf::init(&repo_root, command_name, json);
    // The command span starts here, after repo-root resolution; record the
    // elapsed time since process entry as the `startup` term so the
    // three-term decomposition (startup / prepare / body) is complete.
    perf::log_event(
        "startup",
        process_start.elapsed().as_secs_f64() * 1000.0,
        "ok",
        serde_json::json!({ "command": command_name }),
    );
    let _command_span = perf::span_for_command(command_name);
    let started = Instant::now();

    // Dispatch-side rendezvous classification (plan D7): plain `check` is a
    // shared holder; `check --fix` is exclusive (multi-file journal
    // materialization). search/list/summary/none take NO dispatch-level
    // lock — the index tier wires its own choreography — and `--clear-cache`
    // is excluded here because CacheStore::clear() already takes the
    // exclusive rendezvous itself; wrapping it would contend with our own
    // process. update-check/perf are exempt. Acquisition waits a bounded
    // ~10 s; on timeout the command proceeds WITHOUT the lock after one
    // stderr line — the floor is exactly today's behavior. An unresolvable
    // common dir proceeds silently: there is no store location to contend
    // through, and each subsystem owns its own diagnostic budget.
    let _dispatch_rendezvous =
        if let Some(Commands::Check { fix, clear_cache, .. }) = command.as_ref() {
            if !clear_cache {
                acquire_dispatch_rendezvous(!*fix)
            } else {
                None
            }
        } else {
            None
        };

    let result = match command {
        Some(Commands::Check {
            globs,
            no_exit_code,
            fix,
            fix_dry_run,
            print_applied,
            clear_cache,
        }) => {
            if fix && !matches!(source, index::DocSource::WorkingTree) {
                eprintln!("error: --fix requires --source=worktree");
                return Ok(2);
            }
            // `--clear-cache` (plan decision 8): a best-effort delete that
            // short-circuits the check entirely — the cache is constructed
            // exactly as a run constructs it, cleared, the deleted path
            // printed, and exit 0 returned regardless.
            if clear_cache {
                return commands::check::clear_cache();
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
        None if query.is_some() => "search",
        None => "help",
    }
}

/// Acquire the dispatch-level rendezvous lock for one command run (plan D7).
/// `true` requests the shared mode (plain `check`), `false` the exclusive
/// mode (`--fix`). A bounded-wait timeout (`WouldBlock`) or any other
/// acquisition error emits exactly one stderr line and returns `None`: the
/// command proceeds unordered, which is precisely today's behavior. An
/// unresolvable common dir returns `None` silently — there is no store
/// location to contend through, and every subsystem owns its own
/// diagnostic budget.
fn acquire_dispatch_rendezvous(
    want_shared: bool,
) -> Option<crate::cache::rendezvous::RendezvousGuard> {
    let Ok(common) = crate::git::common_dir() else {
        return None;
    };
    let acquired = if want_shared {
        crate::cache::rendezvous::acquire_shared(&common)
    } else {
        crate::cache::rendezvous::acquire_exclusive(&common)
    };
    match acquired {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("warning: rendezvous lock unavailable ({e}); proceeding without it");
            None
        }
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
