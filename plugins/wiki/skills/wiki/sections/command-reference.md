# Command reference

Flat lookup. For the day-to-day flow see the other sections; this is the surface.

## Subcommands

| Command | Purpose |
|---|---|
| `wiki [query]` | Default. Ranked search over titles/summaries. |
| `wiki list [--tag T] [--limit N]` | All pages with title, aliases, tags, path. |
| `wiki summary <title\|alias\|path>` | Print a page's summary (`{title,file,summary}` under `--format json`); reads stdin if omitted. |
| `wiki check [globs…]` | Validate links and frontmatter, and classify line-range links against their git-derived anchor epoch. |

## `check` flags

| Flag | Effect |
|---|---|
| `--fix` | Relocate drifted line-range links to where their certified content moved, route broken targets through the rename machinery, and initialize `links-reviewed:` on field-less pages carrying line-range links. Requires `--source=worktree`. |
| `--fix-dry-run` | Preview what `--fix` would rewrite; no mutation. Requires `--fix`. |
| `--print-applied` | stdout = one repo-relative path per file this run rewrote; fix/skip summary and diagnostics → stderr. Requires `--fix`. |
| `--no-exit-code` | Exit 0 even with errors (report-only). |

## Global flags

| Flag | Effect |
|---|---|
| `-l N` / `-o N` | Search limit (default 3) / offset. |
| `--format json` | Structured output. |
| `--source worktree\|index\|head` | Snapshot to read. Default `worktree`. `--fix` needs `worktree`. |
| `-v`, `--version` | Print CLI version. |
| `--perf` (or `WIKI_PERF=1`) | Per-event timings to stderr. |

## Anchor grammar

`path#Lstart-Lend` (line range) or bare `path` (whole file). Paths are repo-relative-resolvable per markdown rules. A line-range link's classification is decided against the certified copy of the file at the page's anchor commit — the commit where its `links-reviewed:` value last changed.

## Certification

`links-reviewed:` is a frontmatter field whose value is the page's anchor epoch. It is a **whole-page** certification: one field certifies every line-range link on the page. Changing the value re-certifies the page at the current state — after review — and is the remedy for in-place drift. `--fix` never bumps it for you.

## Reserved titles

`title` and aliases may not be: `check`, `list`, `summary`.
