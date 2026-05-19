# Git Hook Setup

A single pre-commit `wiki` concern with two phases: it auto-repairs drifted wiki links during the commit, then creates and stages git mesh coverage for any uncovered fragment links — all before the commit lands.

---

## Design

Wiki validation runs as one sub-script (`pre-commit.wiki.sh`) with two phases, because the two error classes have different timing requirements but share the same pre-commit invocation:

| Error class | Handling | Phase | Action |
|---|---|---|---|
| Drifted links, anchors, frontmatter | **Auto-fixed, non-blocking** | 1 | `wiki check --fix` rewrites in place, re-stage |
| `mesh_uncovered` (missing git mesh coverage) | **Fail-closed** | 2 | `wiki scaffold --print-applied` creates/renames meshes, stage exactly those |

**Why link repair is non-blocking and runs first.** Most wiki link errors are mechanical drift: a renamed target, a shifted line range, an alias that should resolve to a canonical title. `wiki check --fix` rewrites these deterministically, and the hook re-stages the rewritten files so the commit carries correct links without a manual edit-and-retry cycle. `--no-exit-code` keeps the commit from being rejected for drift the tool already repaired. Errors that `--fix` cannot resolve (a deleted target, an ambiguous rename) are reported but still do not block — phase 1 is a local development guard, not the repository's hard commit gate.

**Why mesh coverage is fail-closed and runs second.** Only a pre-commit hook can stage the freshly-created `.mesh/` files into the commit being made, and only a pre-commit hook can abort the commit when coverage is missing. Phase 2 runs after phase 1 so repaired links are already staged before coverage is computed. `wiki scaffold` self-discovers every uncovered fragment link across the corpus, is idempotent, and fails closed on its own when `git-mesh` is unavailable — so no separate `wiki check`/`jq` pre-filter is needed. A non-zero `wiki scaffold` exit (git-mesh missing, or a genuine `git mesh add` failure) aborts the commit.

---

## The hook

`.githooks/pre-commit.wiki.sh`:

```bash
#!/bin/bash
# Single wiki concern, two phases:
#   1. Auto-fix drifted wiki links/anchors/frontmatter on the working tree and
#      re-stage the fixed .md files (non-blocking — `--no-exit-code`).
#   2. Create git mesh coverage for any uncovered fragment links and stage
#      exactly the meshes this run created/renamed (fail-closed).
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

# ── Phase 1: auto-fix links/anchors/frontmatter, re-stage ────────────────────
"$WIKI_BIN" check --fix --no-exit-code --no-mesh --source=worktree

WIKI_FIXED=$(git diff --name-only --diff-filter=d -- '*.md')
if [ -n "$WIKI_FIXED" ]; then
    # shellcheck disable=SC2086
    git add $WIKI_FIXED
    echo "Re-staged wiki-fixed files:"
    echo "$WIKI_FIXED"
fi

# ── Phase 2: mesh coverage (fail-closed) ─────────────────────────────────────
APPLIED=$("$WIKI_BIN" scaffold --print-applied) || {
    echo "wiki scaffold failed (fail-closed); aborting commit" >&2
    exit 1
}
if [ -n "$APPLIED" ]; then
    while IFS= read -r mesh_path; do
        [ -n "$mesh_path" ] && git add -- "$mesh_path"
    done <<< "$APPLIED"
    echo "Staged scaffolded meshes:"
    echo "$APPLIED"
fi
exit 0
```

### Phase 1 flags — each is load-bearing

- `--fix` rewrites drifted links and anchors in place; it requires `--source=worktree` (you can only rewrite files read from the worktree, not the index or HEAD).
- `--no-exit-code` keeps the commit from being rejected — the hook repairs drift rather than blocking on it.
- `--no-mesh` omits the mesh-coverage check from the repair phase; coverage is enforced fail-closed in phase 2.

After `--fix` runs, the rewritten `.md` files are unstaged worktree changes; the hook re-stages them so the fixes land in the same commit.

**Why check all wiki pages, not just staged files.** A staged edit can break a wikilink on an unstaged page, or introduce a title collision with a page that was not touched. Checking the entire corpus catches these cross-page failures and lets `--fix` repair them.

### Phase 2 — `wiki scaffold --print-applied`

`wiki scaffold` (no globs) walks the whole corpus, builds its own coverage index, and creates a `git mesh` for every uncovered fragment link (anchors only, no why). It is idempotent — already-covered links are skipped — so it can run on every commit.

`--print-applied` prints **one repo-relative path per mesh this run created or renamed** to stdout, and routes all advisories and rename notices to **stderr** (shown on the terminal). The hook stages **exactly** those paths — never a blanket `git add .mesh/` — so unrelated working-tree `.mesh/` edits are not swept into the commit.

**Fail-closed.** A non-zero `wiki scaffold` exit means `git-mesh` is unavailable or a genuine `git mesh add` failed; the `|| { … exit 1; }` aborts the commit. Conditions scaffold handles itself and exits 0 for (so the commit proceeds): a drifted/invalid anchor it drops with an advisory, and a slug **path collision** with a pre-existing ancestor mesh — scaffold renames the blocking mesh to `<blocker>/<derived-leaf>` (or `<blocker>/index`) so both can coexist, prints the renamed blocker's new path on stdout for staging, and notes the rename on stderr.

> Mesh rename-on-collision requires **git-mesh ≥ 1.0.83** (earlier versions evaluated the prefix-collision guard against HEAD, so an uncommitted rename did not free the path within the same pre-commit run).

Use `wiki scaffold --dry-run` to preview created meshes and any planned renames without mutating `.mesh/` before committing.

If `wiki` is not installed (`command -v` fails), the hook silently passes — wiki validation is a local development guard, not a CI gate.
