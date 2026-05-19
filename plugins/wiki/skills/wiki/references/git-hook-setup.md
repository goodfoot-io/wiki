# Git Hook Setup

A two-phase git hook configuration that auto-repairs drifted wiki links during the commit, then scaffolds mesh coverage for newly introduced fragment links after the commit lands.

---

## Design

Wiki validation is split across two hooks because the two error classes have different timing requirements:

| Error class | Handling | Hook | Action |
|---|---|---|---|
| Drifted links, anchors, frontmatter | **Auto-fixed, non-blocking** | `pre-commit` | Rewrite in place, re-stage |
| `mesh_uncovered` (missing git mesh coverage) | **Deferred** | `post-commit` | Auto-scaffold |

**Why link repair is non-blocking and runs in pre-commit.** Most wiki link errors are mechanical drift: a renamed target, a shifted line range, an alias that should resolve to a canonical title. `wiki check --fix` rewrites these deterministically, and the hook re-stages the rewritten files so the commit carries correct links without a manual edit-and-retry cycle. `--no-exit-code` keeps the commit from being rejected for drift the tool already repaired. Errors that `--fix` cannot resolve (a deleted target, an ambiguous rename) are reported but still do not block — wiki validation here is a local development guard, not the repository's hard commit gate.

**Why mesh coverage is deferred.** Mesh scaffolding generates `git mesh add` commands that require human review: anchors must be consolidated into meaningful meshes, and each mesh needs a real `why` sentence. The scaffold output is a starting point, not a finished product. Running it post-commit prints the commands so the committer can act on them without blocking the commit.

---

## Pre-Commit: Auto-Fix and Re-Stage

```bash
#!/bin/bash
set -e

WIKI_BIN=$(command -v wiki || true)
if [ -n "$WIKI_BIN" ]; then
    "$WIKI_BIN" check --fix --no-exit-code --no-mesh --source=worktree
    mapfile -t WIKI_FIXED < <(git diff --name-only --diff-filter=d -- '*.md')
    if [ ${#WIKI_FIXED[@]} -gt 0 ]; then
        git add "${WIKI_FIXED[@]}"
        echo "Re-staged wiki-fixed files:"
        printf '%s\n' "${WIKI_FIXED[@]}"
    fi
fi
```

Each flag is load-bearing:

- `--fix` rewrites drifted links and anchors in place; it requires `--source=worktree` (you can only rewrite files read from the worktree, not the index or HEAD).
- `--no-exit-code` keeps the commit from being rejected — the hook repairs drift rather than blocking on it.
- `--no-mesh` skips the git mesh coverage check so mesh scaffolding is deferred to post-commit and not required before the commit exists.

After `--fix` runs, the rewritten `.md` files are unstaged worktree changes; the hook re-stages them so the fixes land in the same commit. `mapfile -t` + `git add "${WIKI_FIXED[@]}"` passes each path as one argument, so a page path containing whitespace is re-staged intact.

**Why check all wiki pages, not just staged files.** A staged edit can break a wikilink on an unstaged page, or introduce a title collision with a page that was not touched. Checking the entire corpus catches these cross-page failures and lets `--fix` repair them.

If `wiki` is not installed (`command -v` fails), the hook silently passes — wiki validation is a local development guard, not a CI gate.

---

## Post-Commit: Scaffold Mesh Coverage

```bash
WIKI_BIN=$(command -v wiki || true)
if [ -n "$WIKI_BIN" ]; then
  WIKI_JSON=$("$WIKI_BIN" check --format json --no-exit-code 2>&1) || true

  if command -v jq >/dev/null 2>&1 && [ -n "$WIKI_JSON" ]; then
    mapfile -t MESH_FILES < <(echo "$WIKI_JSON" \
      | jq -r '[.errors[] | select(.kind == "mesh_uncovered") | .file] | unique | .[]' \
      2>/dev/null || true)
    if [ ${#MESH_FILES[@]} -gt 0 ]; then
      "$WIKI_BIN" scaffold "${MESH_FILES[@]}"
    fi
  fi
fi
```

`--format json` produces structured output that `jq` can filter. `--no-exit-code` ensures the post-commit never fails — pre-commit has already auto-repaired fixable link drift, so what reaches here is mesh coverage gaps (expected for newly committed pages) and any link errors `--fix` could not resolve.

**The jq filter** selects errors where `kind == "mesh_uncovered"`, extracts the `file` field, and deduplicates. All collected files are passed to `wiki scaffold` in a single invocation.

**Why `mapfile` + a quoted array.** Reading one path per line into an array with `mapfile -t` and expanding it as `"${MESH_FILES[@]}"` passes each path as exactly one argument, so a wiki page path containing whitespace (`docs/My Page.md`) reaches `scaffold` intact instead of being word-split into broken fragments. `${#MESH_FILES[@]} -gt 0` is then an unambiguous emptiness test. A newline-per-path delimiter is safe because wiki page paths never contain newlines.

**The scaffold output** is a shell script of `git mesh add` and `git mesh why` commands. It is printed to stdout for the committer to review, consolidate, and commit separately — it is not executed automatically.
