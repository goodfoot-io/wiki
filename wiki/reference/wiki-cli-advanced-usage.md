---
title: Wiki CLI Advanced Usage
summary: Advanced wiki CLI usage including glob targeting, JSON output, and stdin/path input.
tags:
  - reference
links-reviewed: 2
---

# Wiki CLI Advanced Usage

## Listing Pages

[`wiki list`](/packages/cli/src/commands/list.rs#L17-L77) enumerates all pages with their title, summary, aliases, tags, and file path.

```bash
# List every page
wiki list

# Filter by tag
wiki list --tag api
```

## Keeping Fragment Links Honest

Run `wiki check --fix` to repair mechanical drift automatically:

```bash
# Relocate moved links, route broken targets through renames,
# initialize links-reviewed on field-less pages
wiki check --fix
```

`--fix` only rewrites what it can resolve unambiguously: links whose certified content moved are re-pointed, and pages with line-range links but no `links-reviewed:` field get the field initialized. In-place drift, ambiguous moves, and unverifiable links are skipped with a named reason — see the `resolving-skipped-fixes` skill section.

`wiki check` memoizes its history walks in a disposable cache under the repository's git common directory; [the `--clear-cache` flag](/packages/cli/src/main.rs#L326-L327) deletes it and exits 0:

```bash
# Best-effort delete of the anchor cache directory; prints the path
wiki check --clear-cache
```

The cache is safe to delete at any time — it holds nothing that cannot be recomputed, and `WIKI_ANCHOR_CACHE=0` disables it for a single run.

## Stdin and Path Input

[`wiki`](/packages/cli/src/commands/search.rs#L9-L43) and [`wiki summary`](/packages/cli/src/commands/summary.rs#L72-L100) each accept a file path in addition to a page title or alias:

```bash
# Path argument
wiki summary wiki/my-page.md
wiki wiki/my-page.md

# Single line from stdin — reads when the argument is omitted
echo "wiki/my-page.md" | wiki summary
echo "My Page"         | wiki summary
echo "wiki/my-page.md" | wiki

# Multiple lines from stdin — processes each, exits with the worst code seen
ls wiki/*.md | wiki summary
printf "wiki/page-a.md\nwiki/page-b.md\n" | wiki summary
ls wiki/*.md | wiki   # prints each page separated by a blank line and ---
```

A string is treated as a path when it contains `/` or ends with `.md`; otherwise it is resolved as a title or alias. Relative paths are resolved against the current working directory first, then against the repository root.

When multiple inputs are provided via stdin, the exit code reflects the worst result across all inputs: 0 if all succeeded or returned no matches, 1 if any command reported a business-logic failure, 2 if any runtime error occurred.

## Targeting Specific Files

All commands accept explicit glob patterns instead of scanning [the current working directory](/packages/cli/src/main.rs#L290-L293):

```bash
wiki check wiki/some-section/**/*.md
```

## Excluding Non-Wiki Files

[`.wiki/.wikiignore`](/packages/cli/src/wikiignore.rs) excludes paths from `wiki check` entirely — before frontmatter parsing, link validation, or line-range drift classification ever runs. It lives at `.wiki/.wikiignore` (repo root), uses gitignore syntax, and patterns are matched relative to the repository root. Every discovery path — [`discover_files`](/packages/cli/src/commands/mod.rs#L231-L305), [`discover_files_by_parallel_walk`](/packages/cli/src/commands/mod.rs#L570-L642), and [`discover_files_by_glob_in_source`](/packages/cli/src/commands/mod.rs#L487-L522) — consults it before any file is treated as a wiki page, for `--source=worktree`, `index`, and `head` alike, and regardless of whether an explicit glob is passed.

This is the escape hatch for Markdown that lives in the repo but isn't a wiki page — agent instructions, changelogs, vendored docs — so it never needs frontmatter and never counts against link or drift validation.

```bash
# .wiki/.wikiignore
CLAUDE.md
```

With that entry in place, `wiki check CLAUDE.md` (or a glob that happens to match it, e.g. `wiki check '**/*.md'`) skips the file silently instead of failing with a missing-frontmatter diagnostic.

## JSON Output

Every command accepts [`--format json`](/packages/cli/src/main.rs#L50-L52) for scripting:

```bash
wiki check --format json
wiki list --format json
```

The JSON schema mirrors the human-readable output: `check` emits an `errors` array and `list` emits a page-result array.

### Command-by-Command Output

The `wiki` CLI uses `--format json`, not `--json`.

#### `wiki [query]`

Text output:

```text
# Authorization
## wiki/security/authorization.md
How auth decisions are made across the system.

Matched snippets:
- L12: The **authorization** layer runs after identity resolution.
```

JSON output:

```json
[
  {
    "title": "Authorization",
    "file": "/repo/wiki/security/authorization.md",
    "summary": "How auth decisions are made across the system.",
    "snippets": [
      {
        "line": 12,
        "text": "The **authorization** layer runs after identity resolution."
      }
    ]
  }
]
```

If no results are found, text output is empty and JSON output is `[]`.

#### `wiki check`

Text output:

```text
Error: Link Drift
- wiki/architecture/wiki-cli.md:37
- content at `packages/cli/src/frontmatter.rs#L48-L51` changed since the anchor epoch — bump `links-reviewed:` after reviewing it
```

JSON output:

```json
{
  "errors": [
    {
      "file": "wiki/architecture/wiki-cli.md",
      "kind": "link_drift",
      "line": 37,
      "message": "content at `packages/cli/src/frontmatter.rs#L48-L51` changed since the anchor epoch — bump `links-reviewed:` after reviewing it"
    }
  ]
}
```

`kind` identifies the diagnostic class (`link_drift`, `link_broken`, `link_unverified`, frontmatter kinds); `file`, `line`, and `message` mirror the human-readable report.

#### `wiki list`

Text output:

```text
**Authorization** — `/repo/wiki/security/authorization.md`
aliases: `authz` · tags: `security`, `auth`

How auth decisions are made across the system.

---
```

JSON output:

```json
[
  {
    "title": "Authorization",
    "aliases": ["authz"],
    "tags": ["security", "auth"],
    "summary": "How auth decisions are made across the system.",
    "file": "/repo/wiki/security/authorization.md"
  }
]
```

#### `wiki summary [title|path]`

Text output:

```text
# Authorization
## wiki/security/authorization.md
How auth decisions are made across the system.
```

JSON output:

```json
{
  "title": "Authorization",
  "file": "/repo/wiki/security/authorization.md",
  "summary": "How auth decisions are made across the system."
}
```

## Exit Codes

All commands use a consistent three-value exit code convention:

| Code | Meaning |
|------|---------|
| 0 | Success (or success with non-fatal warnings) |
| 1 | Validation / business-logic errors found for commands that use that state |
| 2 | Runtime or system error |
