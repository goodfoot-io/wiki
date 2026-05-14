mod commands;
mod frontmatter;
mod git;
mod headings;
mod index;
mod parser;
mod perf;
#[cfg(test)]
mod test_support;
mod version;

use std::io::{self, BufRead, IsTerminal};
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use miette::Result;

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
    long_about = "wiki - Read and maintain wiki pages\n\nPass a query to search wiki pages with weighted ranking:\n  wiki [query]\n\nWith no arguments, wiki prints help and the wiki README when available.\n\nStdin is read when no argument is given for commands that accept it:\n  echo wiki/page.md | wiki summary\n\nCommand names (check, links, list, summary, refs, hook, install) are reserved and cannot be used as page titles.\n\nUse `--root <path>` to point at a wiki root other than the current working directory.",
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

    /// Wiki root directory. Defaults to the current working directory.
    #[arg(long = "root", value_name = "PATH", global = true)]
    root: Option<PathBuf>,

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
    /// covered by a `git mesh` that anchors both the wiki file and the
    /// link target. `git mesh` must be installed; missing the binary
    /// fails the check.
    Check {
        /// Glob patterns to match wiki pages (default: $WIKI_ROOT/**/*.md)
        #[arg(value_name = "glob")]
        globs: Vec<String>,
        /// Exit 0 even when validation errors are found (report-only mode)
        #[arg(long = "no-exit-code")]
        no_exit_code: bool,
        /// Skip the git mesh coverage check (useful when `git mesh check` runs separately)
        #[arg(long = "no-mesh")]
        no_mesh: bool,
        /// Rewrite drifted links and anchors in place (requires --source=worktree).
        #[arg(long = "fix")]
        fix: bool,
        /// Print what would be rewritten without modifying any files (requires --fix).
        #[arg(long = "fix-dry-run", requires = "fix")]
        fix_dry_run: bool,
    },

    /// Find wiki pages that link to the given target.
    ///
    /// Resolves wiki-page targets by title, alias, or file path, and resolves
    /// file targets by repo-relative path. Path-like inputs may match both and
    /// return a unified search-style result set with snippets. Reads from stdin
    /// when the argument is omitted.
    Links {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "target")]
        target: Option<String>,
    },

    /// Run `wiki check` on the written/edited file path from a PostToolUse
    /// event and emit a systemMessage when validation errors remain.
    ///
    /// Use directly as the "command" value in a PostToolUse hook definition.
    Hook,

    /// List all wiki pages with metadata (title, aliases, tags, file path).
    ///
    /// Optionally filter by tag.
    List {
        /// Filter pages by tag
        #[arg(long = "tag", value_name = "tag")]
        tag: Option<String>,
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

    /// Print metadata for all fragment-linked pages referenced by a wiki page.
    ///
    /// Reads from stdin when the argument is omitted.
    Refs {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "title|path")]
        title: Option<String>,
    },

    /// Install the wiki integration into an external tool's config home.
    Install {
        /// Install the Codex integration.
        #[arg(long = "codex")]
        codex: bool,

        /// Print friendly Claude Code setup instructions (informational only).
        #[arg(long = "claude")]
        claude: bool,

        /// Overwrite locally modified managed files after recording a backup.
        #[arg(long = "force")]
        force: bool,

        /// Print the planned file changes without writing.
        #[arg(long = "dry-run")]
        dry_run: bool,

        /// Override $CODEX_HOME and ~/.codex.
        #[arg(long = "codex-home", value_name = "PATH")]
        codex_home: Option<PathBuf>,

        /// Git ref (branch, tag, or SHA) to install from.
        #[arg(long = "ref", value_name = "REF", default_value = "main")]
        git_ref: String,
    },

    /// Generate a shell script of `git mesh add` / `git mesh why` commands
    /// for every fragment link found in the given files or globs.
    Scaffold {
        /// Wiki page files or glob patterns (required)
        #[arg(value_name = "glob", num_args = 1..)]
        globs: Vec<String>,
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

    let result = run(
        cli.command,
        cli.query,
        cli.limit,
        cli.offset,
        cli.root,
        json,
        source,
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
    _root: Option<PathBuf>,
    json: bool,
    source: index::DocSource,
) -> Result<i32> {
    let repo_root = git::repo_root()?;

    let command_name = command_name(command.as_ref(), query.as_deref());
    perf::init(&repo_root, command_name, json);
    let _command_span = perf::span_for_command(command_name);
    let started = Instant::now();

    let result = match command {
        Some(Commands::Check {
            globs,
            no_exit_code,
            no_mesh,
            fix,
            fix_dry_run,
        }) => {
            if fix && !matches!(source, index::DocSource::WorkingTree) {
                eprintln!("error: --fix requires --source=worktree");
                return Ok(2);
            }
            commands::check::run(
                &globs,
                json,
                &repo_root,
                no_exit_code,
                no_mesh,
                source,
                fix,
                fix_dry_run,
            )
        }
        Some(Commands::Links { target }) => {
            let inputs = resolve_inputs(target, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::links::run(input, json, &repo_root, source),
                false,
            )
        }
        Some(Commands::Hook) => {
            let lines = read_stdin_lines();
            let input = lines.join("\n");
            commands::hook_check::run(&input, &repo_root, source)
        }
        Some(Commands::List { tag }) => {
            commands::list::run(&[], tag.as_deref(), json, &repo_root, source)
        }
        Some(Commands::Summary { title }) => {
            let inputs = resolve_inputs(title, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::summary::run(input, json, &repo_root, source),
                false,
            )
        }
        Some(Commands::Refs { title }) => {
            let inputs = resolve_inputs(title, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::refs::run(input, json, &repo_root, source),
                false,
            )
        }
        Some(Commands::Install {
            codex,
            claude,
            force,
            dry_run,
            codex_home,
            git_ref,
        }) => commands::install::run(
            codex,
            claude,
            force,
            dry_run,
            codex_home.as_deref(),
            &git_ref,
        ),
        Some(Commands::Scaffold { globs }) => {
            commands::mesh::scaffold::run(&globs, json, &repo_root, source)
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
        Some(Commands::Links { .. }) => "links",
        Some(Commands::Hook) => "hook",
        Some(Commands::List { .. }) => "list",
        Some(Commands::Summary { .. }) => "summary",
        Some(Commands::Refs { .. }) => "refs",
        Some(Commands::Install { .. }) => "install",
        Some(Commands::Scaffold { .. }) => "scaffold",
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

    #[test]
    fn parses_root_flag() {
        let cli = Cli::try_parse_from(["wiki", "--root", "/tmp/wiki", "query"]).expect("parse");
        assert_eq!(cli.root.as_deref(), Some(std::path::Path::new("/tmp/wiki")));
    }
}
