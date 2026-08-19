---
title: Wiki CLI Advanced Usage
summary: Advanced wiki CLI usage including glob targeting, JSON output, and stdin/path input.
tags:
  - reference
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

## Keeping Fragment Links Pinned

Run `wiki check --fix` to automatically pin unpinned fragment links:

```bash
# Pin all unpinned links in the wiki to their latest commit SHA
wiki check --fix
```

`--fix` only touches links that have no SHA (`missing_sha`). Already-pinned links are left unchanged.

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

All commands accept explicit glob patterns instead of scanning [the current working directory](/packages/cli/src/main.rs#L384-L387):

```bash
wiki check wiki/some-section/**/*.md
```

## Excluding Non-Wiki Files

[`.wiki/.wikiignore`](/packages/cli/src/wikiignore.rs) excludes paths from `wiki check` entirely — before frontmatter parsing, link validation, or line-range drift classification ever runs. It lives at `.wiki/.wikiignore` (repo root), uses gitignore syntax, and patterns are matched relative to the repository root. Every discovery path — [`discover_files`](/packages/cli/src/commands/mod.rs#L236-L310), [`discover_files_by_parallel_walk`](/packages/cli/src/commands/mod.rs#L575-L647), and [`discover_files_by_glob_in_source`](/packages/cli/src/commands/mod.rs#L492-L527) — consults it before any file is treated as a wiki page, for `--source=worktree`, `index`, and `head` alike, and regardless of whether an explicit glob is passed.

This is the escape hatch for Markdown that lives in the repo but isn't a wiki page — agent instructions, changelogs, vendored docs — so it never needs frontmatter and never counts against link or drift validation.

```bash
# .wiki/.wikiignore
CLAUDE.md
```

With that entry in place, `wiki check CLAUDE.md` (or a glob that happens to match it, e.g. `wiki check '**/*.md'`) skips the file silently instead of failing with a missing-frontmatter diagnostic.

## JSON Output

Every command accepts [`--format json`](/packages/cli/src/main.rs#L46-L48) for scripting:

```bash
wiki check --format json
wiki list --format json
```

The JSON schema mirrors the human-readable output: `check` emits a `diagnostics` array and `list` emits a page-result array.

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
**missing_sha** — `/repo/wiki/page.md:8`
Fragment link `packages/cli/src/index.rs` has no pinned SHA. Run `wiki check --fix` to add one automatically.
```

JSON output:

```json
[
  {
    "kind": "missing_sha",
    "file": "/repo/wiki/page.md",
    "line": 8,
    "message": "Fragment link `packages/cli/src/index.rs` has no pinned SHA. Run `wiki check --fix` to add one automatically."
  }
]
```

#### `wiki pin`

Text output:

```text
`/repo/wiki/page.md:8` — `packages/cli/src/index.rs`
`` → ``
```

JSON output:

```json
[
  {
    "wiki_file": "/repo/wiki/page.md",
    "source_line": 8,
    "referenced_path": "packages/cli/src/index.rs",
    "old_sha": "abc1234",
    "new_sha": "def5678",
    "action": "refreshed"
  }
]
```

#### `wiki extract`

Text output:

```text
**Authorization** — How auth decisions are made across the system.
**Identity** — How users and service principals are resolved.
```

JSON output:

```json
[
  {
    "title": "Authorization",
    "summary": "How auth decisions are made across the system.",
    "file": "/repo/wiki/security/authorization.md"
  },
  {
    "title": "Identity",
    "summary": "How users and service principals are resolved.",
    "file": "/repo/wiki/security/identity.md"
  }
]
```

If no wikilinks are found, text output is empty and JSON output is `[]`.

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
