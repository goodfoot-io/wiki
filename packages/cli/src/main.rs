mod commands;
mod frontmatter;
mod git;
mod headings;
mod index;
mod parser;
mod perf;
mod render;
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

#[derive(Debug, Parser)]
#[command(
    name = "wiki",
    version = crate::version::VERSION,
    before_help = concat!("wiki ", env!("WIKI_VERSION"), "\n"),
    about = "wiki - Read and maintain wiki pages",
    long_about = "wiki - Read and maintain wiki pages\n\nPass a query to search wiki pages with weighted ranking:\n  wiki [query]\n\nWith no arguments, wiki prints help and the wiki README when available.\n\nStdin is read when no argument is given for commands that accept it:\n  echo wiki/page.md | wiki summary\n\nCommand names (check, links, list, summary, extract, refs, hook, html, install, serve) are reserved and cannot be used as page titles.",
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
    /// Fragment links: pinned SHA present, referenced file exists,
    /// line ranges within bounds. Wikilinks: target title/alias
    /// exists (case-insensitive), unique, heading fragments
    /// resolve. Frontmatter: title required, aliases and tags
    /// valid, no title/alias collisions (case-insensitive).
    /// Defaults to "$WIKI_DIR/**/*.md".
    ///
    /// With --fix, auto-pins unpinned fragment links (missing_sha) by
    /// resolving each referenced file to its latest commit SHA.
    Check {
        /// Glob patterns to match wiki pages (default: $WIKI_DIR/**/*.md)
        #[arg(value_name = "glob")]
        globs: Vec<String>,
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

    /// Extract wikilinks from stdin and print each page's title and summary.
    ///
    /// Reads arbitrary text from stdin, finds all [[wikilink]] references,
    /// and outputs the canonical title and summary for each resolved page.
    /// Deduplicates by title (in order of first appearance). Unresolved
    /// wikilinks are reported to stderr; exit code 1 if any are unresolved.
    /// With --format json, emits [{ title, summary, file }].
    Extract,

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
    /// --format json, emits
    /// { title, file, summary }.
    Summary {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "title|path")]
        title: Option<String>,
    },

    /// Print metadata for all wikilinks referenced by a wiki page.
    ///
    /// Resolves all [[wikilinks]] found in the given page and returns
    /// their title, summary, aliases, and tags. Useful for pre-fetching
    /// tooltip data for all links on a page in one call.
    /// Reads from stdin when the argument is omitted. With --format json,
    /// emits [{ wikilink, title, file, summary, aliases, tags }] for
    /// resolved links and [{ wikilink, error }] for unresolved ones.
    Refs {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "title|path")]
        title: Option<String>,
    },

    /// Render a page to HTML.
    Html {
        /// Page title, alias, or file path
        #[arg(value_name = "title|path")]
        title: String,

        /// Render only the article fragment
        #[arg(long)]
        fragment: bool,

        /// Base URL used for source fragment links in fragment mode
        #[arg(long, value_name = "url")]
        file_base_url: Option<String>,
    },

    /// Install the wiki integration into an external tool's config home.
    ///
    /// Use --codex to install the Codex integration (downloads the latest
    /// plugin assets, installs the wiki skill, and configures a PostToolUse
    /// hook that runs `wiki hook`; repeat runs update the install).
    /// Use --claude to print friendly setup instructions for the Claude Code
    /// plugin marketplace — this mode is informational only and never
    /// touches the filesystem or the network.
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

    /// Start a local web server that renders wiki pages as HTML.
    Serve {
        /// Port to listen on
        #[arg(short = 'p', long, default_value = "8080")]
        port: u16,

        /// Disable live reload over server-sent events
        #[arg(long)]
        no_reload: bool,
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
///
/// When `separate` is true, a `---` divider is printed between items in
/// text output — useful for the print command where each page is a block
/// of markdown that would otherwise run together.
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

    if !json {
        miette::set_hook(Box::new(|_| {
            Box::new(miette::MietteHandlerOpts::new().build())
        }))
        .ok();
    }

    let result = run(cli.command, cli.query, cli.limit, cli.offset, json);

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
) -> Result<i32> {
    let repo_root = git::repo_root()?;
    let command_name = command_name(command.as_ref(), query.as_deref());
    perf::init(&repo_root, command_name, json);
    let started = Instant::now();

    let result = match command {
        Some(Commands::Check { globs }) => commands::check::run(&globs, json, &repo_root),
        Some(Commands::Links { target }) => {
            let inputs = resolve_inputs(target, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::links::run(input, json, &repo_root),
                false,
            )
        }
        Some(Commands::Extract) => {
            let lines = read_stdin_lines();
            if lines.is_empty() {
                return Err(miette::miette!(
                    "no input provided (pipe text containing [[wikilinks]] via stdin)"
                ));
            }
            let input = lines.join("\n");
            commands::extract::run(&input, json, &repo_root)
        }
        Some(Commands::Hook) => {
            let lines = read_stdin_lines();
            let input = lines.join("\n");
            commands::hook_check::run(&input, &repo_root)
        }
        Some(Commands::List { tag }) => commands::list::run(&[], tag.as_deref(), json, &repo_root),
        Some(Commands::Summary { title }) => {
            let inputs = resolve_inputs(title, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::summary::run(input, json, &repo_root),
                false,
            )
        }
        Some(Commands::Refs { title }) => {
            let inputs = resolve_inputs(title, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::refs::run(input, json, &repo_root),
                false,
            )
        }
        Some(Commands::Html {
            title,
            fragment,
            file_base_url,
        }) => commands::html::run(&title, fragment, file_base_url.as_deref(), &repo_root),
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
        Some(Commands::Serve { port, no_reload }) => {
            commands::serve::run(port, no_reload, &repo_root)
        }
        None => match query.as_deref() {
            Some(query) => commands::search::run(query, limit, offset, json, &repo_root),
            None => {
                // No subcommand and no query: print help and the wiki README.
                let mut cmd = <Cli as clap::CommandFactory>::command();
                cmd.print_help().ok();

                let wiki_dir_name =
                    std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string());
                let wiki_dir_path = std::path::PathBuf::from(&wiki_dir_name);
                let wiki_dir = if wiki_dir_path.is_absolute() {
                    wiki_dir_path
                } else {
                    repo_root.join(&wiki_dir_name)
                };
                let readme_path = wiki_dir.join("README.md");

                if readme_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&readme_path) {
                        println!("\n---\n");
                        println!("{}", content.trim_end());
                    }
                } else {
                    println!();
                }
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
        Some(Commands::Extract) => "extract",
        Some(Commands::Hook) => "hook",
        Some(Commands::List { .. }) => "list",
        Some(Commands::Summary { .. }) => "summary",
        Some(Commands::Refs { .. }) => "refs",
        Some(Commands::Html { .. }) => "html",
        Some(Commands::Install { .. }) => "install",
        Some(Commands::Serve { .. }) => "serve",
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
