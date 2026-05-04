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
mod wiki_config;

use std::io::{self, BufRead, IsTerminal};
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use miette::Result;

/// Subcommands that support `-n '*'` (all namespaces) mode.
const SUPPORTED_MULTI_NS: &str = "search, check, links, list, summary, refs";

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
    long_about = "wiki - Read and maintain wiki pages\n\nPass a query to search wiki pages with weighted ranking:\n  wiki [query]\n\nWith no arguments, wiki prints help and the wiki README when available.\n\nStdin is read when no argument is given for commands that accept it:\n  echo wiki/page.md | wiki summary\n\nCommand names (check, links, list, summary, extract, refs, hook, install) are reserved and cannot be used as page titles.\n\nUse `-n '*'` to run a command across all wikis in the repo. Each result is labeled with its namespace. Supported subcommands: search (default query), check, links, list, summary, refs.",
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

    /// Target a peer namespace (or `*` for all) for the default search.
    /// Defaults to the current wiki. For other commands, pass `-n` after
    /// the subcommand (supported on: check, links, list, summary, refs).
    #[arg(short = 'n', long = "namespace", value_name = "NS")]
    namespace: Option<String>,

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
    /// Wikilinks: target title/alias exists (case-insensitive), unique,
    /// heading fragments resolve. Frontmatter: title required, aliases
    /// and tags valid, no title/alias collisions (case-insensitive).
    /// Defaults to all wikis in the repository.
    ///
    /// Always verifies that every fragment link with a line range is
    /// covered by a `git mesh` that anchors both the wiki file and the
    /// link target. `git mesh` must be installed; missing the binary
    /// fails the check.
    ///
    /// Use `-n <name>` to scope the check to a single namespace.
    Check {
        /// Glob patterns to match wiki pages (default: $WIKI_DIR/**/*.md)
        #[arg(value_name = "glob")]
        globs: Vec<String>,
        /// Exit 0 even when validation errors are found (report-only mode)
        #[arg(long = "no-exit-code")]
        no_exit_code: bool,
        /// Skip the git mesh coverage check (useful when `git mesh check` runs separately)
        #[arg(long = "no-mesh")]
        no_mesh: bool,
        /// Target a peer namespace (or `*` for all). Defaults to all wikis.
        #[arg(short = 'n', long = "namespace", value_name = "NS")]
        namespace: Option<String>,
    },

    /// Find wiki pages that link to the given target.
    ///
    /// Resolves wiki-page targets by title, alias, or file path, and resolves
    /// file targets by repo-relative path. Path-like inputs may match both and
    /// return a unified search-style result set with snippets. Reads from stdin
    /// when the argument is omitted.
    ///
    /// By default, searches all wikis in the repo (repo-wide backlinks). Use
    /// `-n <ns>` to scope to a single namespace, or `-n '*'` as an explicit
    /// synonym for the repo-wide default.
    Links {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "target")]
        target: Option<String>,
        /// Target a peer namespace (or `*` for all). Defaults to repo-wide.
        #[arg(short = 'n', long = "namespace", value_name = "NS")]
        namespace: Option<String>,
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
    /// Optionally filter by tag. Use `-n '*'` to list pages across all wikis
    /// in the repo; each row is labeled with its namespace.
    List {
        /// Filter pages by tag
        #[arg(long = "tag", value_name = "tag")]
        tag: Option<String>,
        /// Target a peer namespace (or `*` for all). Defaults to the current wiki.
        #[arg(short = 'n', long = "namespace", value_name = "NS")]
        namespace: Option<String>,
    },

    /// Print the summary of a wiki page.
    ///
    /// Resolves the argument via title, alias, or file path (case-insensitive
    /// for title/alias), then writes the canonical title, absolute path, and
    /// summary to stdout. Reads from stdin when the argument is omitted. With
    /// --format json, emits
    /// { title, file, summary }.
    ///
    /// Use `-n '*'` to search across all wikis in the repo.
    Summary {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "title|path")]
        title: Option<String>,
        /// Target a peer namespace (or `*` for all). Defaults to the current wiki.
        #[arg(short = 'n', long = "namespace", value_name = "NS")]
        namespace: Option<String>,
    },

    /// Print metadata for all wikilinks referenced by a wiki page.
    ///
    /// Resolves all [[wikilinks]] found in the given page and returns
    /// their title, summary, aliases, and tags. Useful for pre-fetching
    /// tooltip data for all links on a page in one call.
    /// Reads from stdin when the argument is omitted. With --format json,
    /// emits [{ wikilink, title, file, summary, aliases, tags }] for
    /// resolved links and [{ wikilink, error }] for unresolved ones.
    ///
    /// Use `-n '*'` to resolve wikilinks across all wikis in the repo.
    Refs {
        /// Page title, alias, or file path; reads from stdin if omitted
        #[arg(value_name = "title|path")]
        title: Option<String>,
        /// Target a peer namespace (or `*` for all). Defaults to the current wiki.
        #[arg(short = 'n', long = "namespace", value_name = "NS")]
        namespace: Option<String>,
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

    /// Generate a shell script of `git mesh add` / `git mesh why` commands
    /// for every fragment link found in the given files or globs.
    /// Operates across all wikis in the repository regardless of the -n flag.
    Scaffold {
        /// Wiki page files or glob patterns (required)
        #[arg(value_name = "glob", num_args = 1..)]
        globs: Vec<String>,
    },

    /// List the current wiki's namespace and each declared peer with its
    /// resolved path and validation status. Exits non-zero if any peer
    /// fails rule 1 (missing wiki.toml) or rule 2 (alias/namespace mismatch).
    Namespaces,

    /// Create a wiki.toml in the current directory.
    ///
    /// With a namespace argument: writes `namespace = "<arg>"`.
    /// Without: writes an empty file (default namespace).
    /// Fails closed if wiki.toml already exists.
    Init {
        /// Namespace name for the new wiki (omit for the default namespace).
        #[arg(value_name = "namespace")]
        namespace: Option<String>,
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
    perf::enable_stderr(cli.perf);

    if !json {
        miette::set_hook(Box::new(|_| {
            Box::new(miette::MietteHandlerOpts::new().build())
        }))
        .ok();
    }

    let namespace = cli
        .namespace
        .clone()
        .or_else(|| subcommand_namespace(&cli.command));

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
        namespace,
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
    namespace: Option<String>,
    json: bool,
    source: index::DocSource,
) -> Result<i32> {
    let repo_root = git::repo_root()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| repo_root.clone());

    // Skip wiki-config loading for subcommands that may run in a directory
    // without a wiki: `install` (no wiki touched), `hook` (silently no-ops on
    // non-wiki files; would otherwise fail-closed on every edit outside a wiki
    // repo), `init` (creates the wiki.toml — can't load what doesn't exist),
    // and `namespaces` (does its own lenient peer-walk so broken peers are
    // reported inline rather than aborting the load).
    let needs_config = !matches!(
        command,
        Some(Commands::Install { .. })
            | Some(Commands::Hook)
            | Some(Commands::Init { .. })
            | Some(Commands::Namespaces)
    );

    let config = if needs_config {
        Some(wiki_config::WikiConfig::load(&cwd, &repo_root)?)
    } else {
        None
    };

    // `@foo query` sugar in the default-query branch.
    let (effective_namespace, effective_query) = match (&command, query.as_deref(), &config) {
        (None, Some(q), Some(cfg)) => apply_at_sugar(q, cfg, namespace.as_deref()),
        _ => (namespace.clone(), query.clone()),
    };

    // Scaffold always operates across all wikis, regardless of -n.
    if let Some(Commands::Scaffold { globs }) = &command {
        let cfg = config.as_ref().ok_or_else(|| {
            miette::miette!(
                "wiki scaffold requires a wiki.toml (no wiki found)"
            )
        })?;
        let wiki_roots: Vec<PathBuf> = cfg.all().map(|w| w.root.clone()).collect();
        let command_name = command_name(command.as_ref(), effective_query.as_deref());
        let perf_root = cfg
            .all()
            .next()
            .map(|w| w.root.as_path())
            .unwrap_or(repo_root.as_path());
        perf::init(perf_root, command_name, json);
        let _command_span = perf::span_for_command(command_name);
        let started = Instant::now();
        let result = commands::mesh::scaffold::run(globs, json, &wiki_roots, &repo_root, source);
        match &result {
            Ok(code) => perf::finish(
                command_name,
                *code,
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
        return result;
    }

    // wiki check defaults to all wikis when -n is not specified
    let effective_namespace = match (&command, &effective_namespace) {
        (Some(Commands::Check { .. }), None) => Some("*".to_string()),
        _ => effective_namespace,
    };

    if effective_namespace.as_deref() == Some("*") {
        let cfg = config.as_ref().ok_or_else(|| {
            miette::miette!(
                "this command does not support multi-namespace (`-n '*'`)"
            )
        })?;
        let targets: Vec<(String, &std::path::Path)> = cfg
            .all()
            .map(|info| {
                (
                    info.namespace.clone().unwrap_or_else(|| "default".into()),
                    info.root.as_path(),
                )
            })
            .collect();
        let command_name = command_name(command.as_ref(), effective_query.as_deref());
        // Use the first discovered wiki's root as the perf root; informational.
        let perf_root = cfg
            .all()
            .next()
            .map(|w| w.root.as_path())
            .unwrap_or(repo_root.as_path());
        perf::init(perf_root, command_name, json);
        let _command_span = perf::span_for_command(command_name);
        let started = Instant::now();
        let result: Result<i32> = match command {
            Some(Commands::Check { globs, no_exit_code, no_mesh, namespace: _ }) => {
                commands::check::run_multi(&globs, json, &targets, &repo_root, no_exit_code, no_mesh, source)
            }
            Some(Commands::Links { target, namespace: _ }) => {
                let inputs = resolve_inputs(target, read_stdin_lines)?;
                run_for_each(
                    inputs,
                    |input| commands::links::run_multi(input, json, &targets, &repo_root, source),
                    false,
                )
            }
            Some(Commands::Summary { title, namespace: _ }) => {
                let inputs = resolve_inputs(title, read_stdin_lines)?;
                run_for_each(
                    inputs,
                    |input| commands::summary::run_multi(input, json, &targets, &repo_root, source),
                    false,
                )
            }
            Some(Commands::Refs { title, namespace: _ }) => {
                let inputs = resolve_inputs(title, read_stdin_lines)?;
                run_for_each(
                    inputs,
                    |input| commands::refs::run_multi(input, json, &targets, &repo_root, Some(cfg), source),
                    false,
                )
            }
            Some(Commands::List { tag, namespace: _ }) => {
                commands::list::run_multi(tag.as_deref(), json, &targets, &repo_root, source)
            }
            None => match effective_query.as_deref() {
                Some(q) => commands::search::run_multi(q, limit, offset, json, &targets, &repo_root, source),
                None => {
                    return Err(miette::miette!(
                        "`-n '*'` requires a query or one of: {SUPPORTED_MULTI_NS}"
                    ));
                }
            },
            _ => {
                return Err(miette::miette!(
                    "command does not support multi-namespace (`-n '*'`); supported: {SUPPORTED_MULTI_NS}"
                ));
            }
        };
        match &result {
            Ok(code) => perf::finish(
                command_name,
                *code,
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
        return result;
    }

    let target = config
        .as_ref()
        .map(|cfg| resolve_target(cfg, effective_namespace.as_deref()))
        .transpose()?;
    let wiki_root_pb = target
        .as_ref()
        .map(|info| info.root.clone())
        .unwrap_or_else(|| repo_root.clone());
    let wiki_root = wiki_root_pb.as_path();

    let command_name = command_name(command.as_ref(), effective_query.as_deref());
    perf::init(wiki_root, command_name, json);
    let _command_span = perf::span_for_command(command_name);
    let started = Instant::now();

    let result = match command {
        Some(Commands::Check { globs, no_exit_code, no_mesh, namespace: _ }) => {
            commands::check::run(&globs, json, wiki_root, &repo_root, config.as_ref(), no_exit_code, no_mesh, source)
        }
        Some(Commands::Links { target, namespace: _ }) => {
            let inputs = resolve_inputs(target, read_stdin_lines)?;
            if effective_namespace.is_none() {
                // No explicit -n: default to repo-wide backlinks across all wikis.
                let cfg = config.as_ref().ok_or_else(|| {
                    miette::miette!("no wiki.toml found; cannot resolve repo-wide links")
                })?;
                let all_targets: Vec<(String, &std::path::Path)> = cfg
                    .all()
                    .map(|info| {
                        (
                            info.namespace.clone().unwrap_or_else(|| "default".into()),
                            info.root.as_path(),
                        )
                    })
                    .collect();
                run_for_each(
                    inputs,
                    |input| commands::links::run_multi(input, json, &all_targets, &repo_root, source),
                    false,
                )
            } else {
                run_for_each(
                    inputs,
                    |input| commands::links::run(input, json, wiki_root, &repo_root, source),
                    false,
                )
            }
        }
        Some(Commands::Extract) => {
            let lines = read_stdin_lines();
            if lines.is_empty() {
                return Err(miette::miette!(
                    "no input provided (pipe text containing [[wikilinks]] via stdin)"
                ));
            }
            let input = lines.join("\n");
            commands::extract::run(&input, json, wiki_root, &repo_root, source)
        }
        Some(Commands::Hook) => {
            let lines = read_stdin_lines();
            let input = lines.join("\n");
            commands::hook_check::run(&input, wiki_root, &repo_root, source)
        }
        Some(Commands::List { tag, namespace: _ }) => {
            commands::list::run(&[], tag.as_deref(), json, wiki_root, &repo_root, source)
        }
        Some(Commands::Summary { title, namespace: _ }) => {
            let inputs = resolve_inputs(title, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::summary::run(input, json, wiki_root, &repo_root, source),
                false,
            )
        }
        Some(Commands::Refs { title, namespace: _ }) => {
            let inputs = resolve_inputs(title, read_stdin_lines)?;
            run_for_each(
                inputs,
                |input| commands::refs::run(input, json, wiki_root, &repo_root, config.as_ref(), source),
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
        Some(Commands::Namespaces) => {
            commands::namespaces::run(&cwd, &repo_root, json)
        }
        Some(Commands::Init { namespace }) => {
            commands::init::run(&cwd, namespace.as_deref())
        }
        // Scaffold is handled before the single-wiki resolution block above,
        // so this arm is unreachable.
        Some(Commands::Scaffold { .. }) => {
            unreachable!("scaffold is dispatched before namespace resolution")
        }
        None => match effective_query.as_deref() {
            Some(query) => {
                commands::search::run(query, limit, offset, json, wiki_root, &repo_root, source)
            }
            None => {
                // No subcommand and no query: print help and the wiki README.
                let mut cmd = <Cli as clap::CommandFactory>::command();
                cmd.print_help().ok();

                let readme_path = wiki_root.join("README.md");

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

/// Resolve `-n NS` to a `WikiInfo` from the loaded config. `None` returns the
/// default-namespace wiki; `Some(name)` looks up the wiki with that namespace.
fn resolve_target(
    cfg: &wiki_config::WikiConfig,
    namespace: Option<&str>,
) -> Result<wiki_config::WikiInfo> {
    cfg.resolve(namespace).cloned()
}

/// Apply `@foo query` sugar: if the query starts with `@<word> ` and `<word>`
/// is a declared peer alias or matches the current namespace, strip it and
/// promote it to the effective namespace.
fn apply_at_sugar(
    q: &str,
    cfg: &wiki_config::WikiConfig,
    explicit_ns: Option<&str>,
) -> (Option<String>, Option<String>) {
    if explicit_ns.is_some() {
        return (explicit_ns.map(str::to_string), Some(q.to_string()));
    }
    let stripped = match q.strip_prefix('@') {
        Some(s) => s,
        None => return (None, Some(q.to_string())),
    };
    let space = match stripped.find(char::is_whitespace) {
        Some(i) => i,
        None => return (None, Some(q.to_string())),
    };
    let word = &stripped[..space];
    let rest = stripped[space + 1..].trim_start().to_string();
    let known = cfg.wikis.contains_key(word);
    if known {
        (Some(word.to_string()), Some(rest))
    } else {
        (None, Some(q.to_string()))
    }
}

/// Extract the per-subcommand `-n` namespace, if the active subcommand carries one.
fn subcommand_namespace(command: &Option<Commands>) -> Option<String> {
    match command {
        Some(Commands::Check { namespace, .. })
        | Some(Commands::Links { namespace, .. })
        | Some(Commands::List { namespace, .. })
        | Some(Commands::Summary { namespace, .. })
        | Some(Commands::Refs { namespace, .. }) => namespace.clone(),
        _ => None,
    }
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
        Some(Commands::Install { .. }) => "install",
        Some(Commands::Scaffold { .. }) => "scaffold",
        Some(Commands::Namespaces) => "namespaces",
        Some(Commands::Init { .. }) => "init",
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
