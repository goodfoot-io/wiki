# Wiki CLI Reference

**When to use this:** reaching past the day-to-day commands in `SKILL.md` — inspecting back-references, paginating search, validating specific files, machine-reading diagnostics, or wiring `wiki` into another tool.

The day-to-day commands (`wiki [query]`, `wiki check`, `wiki scaffold`) are documented in `SKILL.md`; this file covers everything else.

---

## Inspection

```bash
wiki summary "Authorization"      # print a page's summary line
wiki refs   "Authorization"       # every link referenced by the page (forward refs)
wiki links  "Authorization"       # every page that links to the target  (back refs)
wiki list                         # all pages with title, aliases, tags, path
```

## Search pagination

```bash
wiki -l 10 "auth"                 # up to 10 results (default 3)
wiki -l 10 -o 10 "auth"           # next page
```

## Validation flags

```bash
wiki check --root wiki            # scope validation to the wiki/ directory
wiki check --no-mesh              # skip mesh coverage (when git mesh runs separately)
wiki check --no-exit-code         # report-only; exits 0 even with errors
wiki check --format json          # structured diagnostics
wiki check path/to/page.md        # validate specific globs only
```

`--format json` is supported on most subcommands and is the right choice for any script consuming wiki output.

## Document source

```bash
wiki --source worktree check      # default: working tree
wiki --source index    check      # staged content (use in pre-commit hooks)
wiki --source head     check      # latest commit (use in CI)
```

`--source` reads from a different snapshot of the repo without touching the working tree. The `index` source is what the pre-commit hook in `git-hook-setup.md` uses.

## Setup and integration

```bash
wiki install <tool>               # install the wiki integration into an external tool's config home
wiki hook                         # PostToolUse hook entrypoint (reads event JSON from stdin)
```

`wiki hook` is wired through Claude Code's hooks configuration, not invoked by hand. It runs `wiki check` against the file the tool just edited and emits a `systemMessage` if validation fails.

## Global flags

| Flag | Effect |
|---|---|
| `-v`, `--version` | Print the CLI version. |
| `--perf` | Emit per-event timings to stderr (also: `WIKI_PERF=1`). |
| `--format json` | Structured output (subcommand-dependent). |
| `--source <s>` | `worktree` (default) / `index` / `head`. |
| `--root <dir>` | Root directory to scan for wiki pages. |
| `-l <N>` / `-o <N>` | Search result limit / offset. |

## Reserved titles

`title` and `aliases` may not be any of: `check`, `pin`, `stale`, `links`, `list`, `summary`, `print`. The bare `wiki <title>` form would otherwise dispatch to the subcommand instead of the page.
