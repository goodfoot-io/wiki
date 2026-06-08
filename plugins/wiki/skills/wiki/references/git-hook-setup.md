# Git Hook Setup

A single pre-commit `wiki` concern: one `wiki check --fix` invocation auto-repairs drifted wiki links and creates mesh coverage for uncovered fragment links, then stages all touched paths — all before the commit lands.

---

## Design

Wiki validation runs as one sub-script (`pre-commit.wiki.sh`) with a single `wiki check --fix` call, because the two error classes can be resolved in the same pass:

| Error class | Handling | Action |
|---|---|---|
| Drifted links, anchors, frontmatter | **Auto-fixed, non-blocking** | `--fix` rewrites in place, re-stage |
| `mesh_uncovered` (missing mesh coverage) | **Best-effort, non-blocking** | Fix #4 inside `--fix` creates/renames meshes; `--print-applied` routes created paths to stdout for staging |

**Why link repair and mesh coverage run together.** `wiki check --fix` handles both error classes in a single pass: the `--fix` pipeline repairs drifted links and anchors first, then creates mesh coverage for any uncovered fragment links (Fix #4). `--print-applied` routes created/renamed mesh paths to stdout while all advisories and diagnostics go to stderr. `--no-exit-code` keeps the commit from being rejected for any error the tool already handled. Errors that `--fix` cannot resolve (a deleted target, an ambiguous rename) are reported on stderr but do not block — the hook is a local development guard, not the repository's hard commit gate.

**Why `--source=worktree`.** `--fix` rewrites files on disk; it requires reading from the working tree, not the index or HEAD.

---

## The hook

`.githooks/pre-commit.wiki.sh`:

```bash
#!/bin/bash
# Single wiki concern, single invocation:
#   wiki check --fix creates/renames meshes for uncovered fragment links and
#   auto-fixes drifted wiki links/anchors/frontmatter in the working tree.
# --no-exit-code makes this best-effort: the hook never aborts a commit.
# --print-applied routes created/renamed mesh paths to stdout; everything else
# goes to stderr (shown on the terminal).
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

# ── Single-pass: auto-fix + mesh coverage, re-stage all touched paths ─────────
# --fix rewrites in place (requires --source=worktree); --print-applied prints
# created/renamed mesh paths to stdout; --no-exit-code = advisory (best-effort).
APPLIED=$("$WIKI_BIN" check --fix --print-applied --no-exit-code --source=worktree)

WIKI_FIXED=$(git diff --name-only --diff-filter=d -- '*.md')
if [ -n "$WIKI_FIXED" ]; then
    # shellcheck disable=SC2086
    git add $WIKI_FIXED
    echo "Re-staged wiki-fixed files:"
    echo "$WIKI_FIXED"
fi

if [ -n "$APPLIED" ]; then
    while IFS= read -r mesh_path; do
        [ -n "$mesh_path" ] && git add -- "$mesh_path"
    done <<< "$APPLIED"
    echo "Staged scaffolded meshes:"
    echo "$APPLIED"
fi
exit 0
```

### Flags — each is load-bearing

- `--fix` rewrites drifted links and anchors in place **and** creates mesh coverage for uncovered fragment links. Requires `--source=worktree` (you can only rewrite files read from the worktree, not the index or HEAD).
- `--print-applied` prints **one repo-relative path per mesh this run created or renamed** to stdout, and routes all advisories and rename notices to **stderr** (shown on the terminal). The hook stages **exactly** those paths — never a blanket `git add .mesh/` — so unrelated working-tree `.mesh/` edits are not swept into the commit.
- `--no-exit-code` keeps the commit from being rejected — the hook repairs drift and creates coverage rather than blocking on them.

After `--fix` runs, the rewritten `.md` files are unstaged worktree changes; the hook re-stages them so the fixes land in the same commit.

**Why check all wiki pages, not just staged files.** A staged edit can break a wikilink on an unstaged page, or introduce a title collision with a page that was not touched. Checking the entire corpus catches these cross-page failures and lets `--fix` repair them.

### Mesh coverage behavior

`wiki check --fix` walks the whole corpus, builds its own coverage index, and creates a mesh for every uncovered fragment link (anchors only, no why). It is idempotent — already-covered links are skipped — so it can run on every commit.

When a new slug path-collides with a pre-existing ancestor mesh, `--fix` renames the blocking mesh to `<blocker>/<derived-leaf>` (or `<blocker>/index`) so both can coexist, prints the renamed blocker's new path on stdout for staging, and notes the rename on stderr.

Use `wiki check --fix --fix-dry-run` to preview created meshes and any planned renames without mutating `.mesh/` before committing.

If `wiki` is not installed (`command -v` fails), the hook silently passes — wiki validation is a local development guard, not a CI gate.
