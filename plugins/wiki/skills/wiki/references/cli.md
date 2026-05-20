# Wiki CLI Reference

**When to use this:** reaching past the day-to-day commands in `SKILL.md` — inspecting back-references, paginating search, validating specific files, machine-reading diagnostics, or wiring `wiki` into another tool.

The day-to-day commands (`wiki [query]`, `wiki check`, `wiki scaffold`) are documented in `SKILL.md`; this file covers everything else. `wiki scaffold` creates git meshes (anchors only, no why) for every uncovered fragment link; `--dry-run` previews the plan without mutating anything; `--format json` emits structured drafts and is non-mutating.

---

## Inspection

```bash
wiki summary "Authorization"      # print a page's summary line
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

`--source` reads from a different snapshot of the repo without touching the working tree. The pre-commit hook in `git-hook-setup.md` uses `--source=worktree` for its `--fix` phase (you can only rewrite files read from the worktree).

## Scaffold flags

```bash
wiki scaffold --dry-run           # preview created meshes + planned renames; no mutation
wiki scaffold --format json       # structured drafts; non-mutating
wiki scaffold --print-applied     # apply mode: stdout = one repo-relative path per
                                  # created/renamed mesh; advisories → stderr
```

`--print-applied` is the pre-commit integration point: it lets the hook stage **exactly** the meshes this run created or renamed (`git add` each printed path) instead of a blanket `git add .mesh/`. Conflicts with `--dry-run`. When a new slug path-collides with a pre-existing ancestor mesh, scaffold renames the blocker to `<blocker>/<derived-leaf>` (or `<blocker>/index`), prints the blocker's new path for staging, and notes the rename on stderr (requires git-mesh ≥ 1.0.83).

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
