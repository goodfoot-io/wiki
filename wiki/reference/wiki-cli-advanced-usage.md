---
title: Wiki CLI Advanced Usage
summary: Advanced wiki CLI usage including glob targeting, JSON output, and stdin/path input.
tags:
  - reference
---

# Wiki CLI Advanced Usage

## Listing Pages

`wiki list` enumerates all pages with their title, summary, aliases, tags, and file path.

```bash
# List every page
wiki list

# Filter by tag
wiki list --tag api
```

## Finding Incoming Links

`wiki links` shows which pages link to a given target. It accepts page titles, aliases, and file paths, and path-like inputs can return both wiki-page links and fragment-link references in one result set.

```bash
wiki links "My Page"
wiki links wiki/my-page.md
wiki links packages/cli/src/index.rs
```

This is useful to understand what documentation exists for a page or file before changing, renaming, or deleting it.

## Keeping Fragment Links Pinned

Run `wiki check --fix` to automatically pin unpinned fragment links:

```bash
# Pin all unpinned links in the wiki to their latest commit SHA
wiki check --fix
```

`--fix` only touches links that have no SHA (`missing_sha`). Already-pinned links are left unchanged.

## Stdin and Path Input

`wiki`, `wiki summary`, and `wiki links` each accept a file path in addition to a page title or alias:

```bash
# Path argument
wiki summary wiki/my-page.md
wiki links wiki/my-page.md
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

All commands accept explicit glob patterns instead of scanning `WIKI_DIR`:

```bash
wiki check wiki/some-section/**/*.md
```

## JSON Output

Every command accepts `--format json` for scripting:

```bash
wiki check --format json
wiki list --format json
wiki links --format json "My Page"
```

The JSON schema mirrors the human-readable output: `check` emits a `diagnostics` array, and `list` and `links` each emit page-result arrays.

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

#### `wiki links [target]`

Text output:

```text
# Reference Page
## wiki/reference.md
References the target file.

Matched snippets:
- L5: Read [the file](wiki/target.md) directly.
```

JSON output:

```json
[
  {
    "title": "Reference Page",
    "file": "/repo/wiki/reference.md",
    "summary": "References the target file.",
    "snippets": [
      {
        "line": 5,
        "text": "Read [the file](wiki/target.md) directly."
      }
    ]
  }
]
```

If no matches are found, text output is empty and JSON output is `[]`.

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
