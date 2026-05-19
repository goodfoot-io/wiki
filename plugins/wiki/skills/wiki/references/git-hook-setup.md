# Git Hook Setup

A two-phase git hook configuration that auto-repairs drifted wiki links during the commit, then creates and stages git mesh coverage for any newly introduced fragment links — all before the commit lands.

---

## Design

Wiki validation is split across two hooks because the two error classes have different timing requirements:

| Error class | Handling | Hook | Action |
|---|---|---|---|
| Drifted links, anchors, frontmatter | **Auto-fixed, non-blocking** | `pre-commit` (phase 1) | Rewrite in place, re-stage |
| `mesh_uncovered` (missing git mesh coverage) | **Fail-closed** | `pre-commit` (phase 2) | `wiki scaffold` creates meshes, `git add .mesh/` stages them |

**Why link repair is non-blocking and runs first.** Most wiki link errors are mechanical drift: a renamed target, a shifted line range, an alias that should resolve to a canonical title. `wiki check --fix` rewrites these deterministically, and the hook re-stages the rewritten files so the commit carries correct links without a manual edit-and-retry cycle. `--no-exit-code` keeps the commit from being rejected for drift the tool already repaired. Errors that `--fix` cannot resolve (a deleted target, an ambiguous rename) are reported but still do not block — wiki validation here is a local development guard, not the repository's hard commit gate.

**Why mesh coverage is fail-closed in pre-commit.** Only a pre-commit hook can stage the freshly-created `.mesh/` files into the commit being made, and only a pre-commit hook can abort the commit when coverage is missing. `wiki scaffold` creates meshes directly (anchors only, no why) via `git mesh add`, and the hook runs `git add .mesh/` so the created files join the same commit. A non-zero `wiki scaffold` exit aborts the commit. Use `wiki scaffold --dry-run` to preview what will be created before committing.

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
- `--no-mesh` skips the git mesh coverage check entirely — coverage enforcement is handled fail-closed in the separate `pre-commit.wiki-mesh.sh` phase; this flag simply omits the check from the link-repair phase.

After `--fix` runs, the rewritten `.md` files are unstaged worktree changes; the hook re-stages them so the fixes land in the same commit. `mapfile -t` + `git add "${WIKI_FIXED[@]}"` passes each path as one argument, so a page path containing whitespace is re-staged intact.

**Why check all wiki pages, not just staged files.** A staged edit can break a wikilink on an unstaged page, or introduce a title collision with a page that was not touched. Checking the entire corpus catches these cross-page failures and lets `--fix` repair them.

If `wiki` is not installed (`command -v` fails), the hook silently passes — wiki validation is a local development guard, not a CI gate.

---

## Pre-Commit: Scaffold Mesh Coverage

```bash
#!/bin/bash
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

command -v jq >/dev/null 2>&1 || exit 0

WIKI_JSON=$("$WIKI_BIN" check --format json --no-exit-code 2>&1) || true
[ -n "$WIKI_JSON" ] || exit 0

mapfile -t MESH_FILES < <(echo "$WIKI_JSON" \
    | jq -r '[.errors[] | select(.kind == "mesh_uncovered") | .file] | unique | .[]' \
    2>/dev/null || true)
if [ ${#MESH_FILES[@]} -gt 0 ]; then
    "$WIKI_BIN" scaffold "${MESH_FILES[@]}"
    git add .mesh/
fi
exit 0
```

This hook runs after the link auto-fix phase (so repaired links are already staged). It must run in pre-commit so the created `.mesh/` files can be staged into the commit that introduced the fragment links.

`--format json` produces structured output that `jq` can filter. `--no-exit-code` lets the hook parse coverage results without failing on them — the fail-closed behavior comes from `set -e` and `wiki scaffold`'s own non-zero exit on failure.

**The jq filter** selects errors where `kind == "mesh_uncovered"`, extracts the `file` field, and deduplicates. All collected files are passed to `wiki scaffold` in a single invocation.

**Why `mapfile` + a quoted array.** Reading one path per line into an array with `mapfile -t` and expanding it as `"${MESH_FILES[@]}"` passes each path as exactly one argument, so a wiki page path containing whitespace (`docs/My Page.md`) reaches `scaffold` intact instead of being word-split into broken fragments. `${#MESH_FILES[@]} -gt 0` is then an unambiguous emptiness test. A newline-per-path delimiter is safe because wiki page paths never contain newlines.

**`wiki scaffold` creates meshes directly** — it calls `git mesh add <slug> <anchors…>` for each uncovered fragment link (anchors only, no why). Created `.mesh/` files are not auto-staged by `git mesh add`; the `git add .mesh/` line is load-bearing. A non-zero `wiki scaffold` exit aborts the commit via `set -e`.

If `wiki` or `jq` is not installed, the hook silently passes — wiki validation is a local development guard, not a CI gate.
