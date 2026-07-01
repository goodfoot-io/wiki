# Git hook setup

One pre-commit concern: a single `wiki check --fix` call auto-repairs drifted links/anchors/frontmatter *and* creates mesh coverage for uncovered fragment links, then stages everything it touched — before the commit lands. Both error classes resolve in the same pass, so it's one invocation, not two.

It is a **local development guard, not a hard commit gate.** Errors `--fix` can't resolve (deleted target, ambiguous rename) print to stderr but never block the commit (`--no-exit-code`). If `wiki` isn't installed, the hook silently passes.

## The hook

`.githooks/pre-commit.wiki.sh`, reproduced verbatim at [`./examples/pre-commit.wiki.sh`](../examples/pre-commit.wiki.sh) so it can be copied straight into a repo:

```bash
#!/bin/bash
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

# --fix rewrites in place (needs --source=worktree) and creates mesh coverage;
# --print-applied prints created/renamed mesh paths to stdout; --no-exit-code = advisory.
#
# wiki check --fix has no flag reporting which .md files it rewrote (only
# --print-applied's mesh paths are machine-readable), so snapshot every
# tracked .md file's content hash before running --fix and re-stage only
# the ones whose hash changed as a result.
BEFORE_HASHES=$(mktemp)
trap 'rm -f "$BEFORE_HASHES"' EXIT
git ls-files -z -- '*.md' | while IFS= read -r -d '' f; do
    printf '%s %s\n' "$(git hash-object "$f")" "$f"
done > "$BEFORE_HASHES"

APPLIED=$("$WIKI_BIN" check --fix --print-applied --no-exit-code --source=worktree)

WIKI_FIXED=()
while IFS=' ' read -r before_hash f; do
    [ -n "$f" ] || continue
    [ -f "$f" ] || continue
    after_hash=$(git hash-object "$f")
    [ "$after_hash" != "$before_hash" ] && WIKI_FIXED+=("$f")
done < "$BEFORE_HASHES"
rm -f "$BEFORE_HASHES"
trap - EXIT

if [ ${#WIKI_FIXED[@]} -gt 0 ]; then
    git add "${WIKI_FIXED[@]}"
    echo "Re-staged wiki-fixed files:"; printf '%s\n' "${WIKI_FIXED[@]}"
fi

if [ -n "$APPLIED" ]; then
    while IFS= read -r mesh_path; do
        [ -n "$mesh_path" ] && git add -- "$mesh_path"
    done <<< "$APPLIED"
    echo "Staged scaffolded meshes:"; echo "$APPLIED"
fi
exit 0
```

## Wiring it in

Two one-time, per-clone setup steps — the hook itself no longer does either automatically:

1. Copy [`./examples/pre-commit.wiki.sh`](../examples/pre-commit.wiki.sh) to `.githooks/pre-commit.wiki.sh` (or wherever your `core.hooksPath` points) and make it executable.
2. Register the wiki-mesh merge driver:
   ```bash
   git config merge.wiki-mesh.driver 'wiki mesh merge %O %A %B %L'
   ```
   See [Optional merge driver](#optional-merge-driver) below for what this does and why it's per-clone, not committed.

## Why each flag is load-bearing

- **`--fix`** — rewrites drifted links/anchors/frontmatter **and** creates coverage for uncovered links. Requires `--source=worktree` (you can only rewrite files read from the worktree).
- **`--print-applied`** — prints one repo-relative path per mesh this run created or renamed to **stdout** (advisories/rename notices → stderr). The hook stages **exactly** those paths — never a blanket `git add .wiki/` — so unrelated working-tree `.wiki/` edits aren't swept in.
- **`--no-exit-code`** — repair-and-continue instead of rejecting the commit.

## Why the `.md` re-stage is hash-based, not a blanket `git diff`

`wiki check --fix` has no flag that reports *which page files it rewrote* — only mesh paths are machine-readable, via `--print-applied`. An earlier version of this hook used `git diff --name-only --diff-filter=d -- '*.md'` after `--fix` ran, which is **not** scoped to files `--fix` touched — it's every modified `.md` file anywhere in the working tree at that moment. An unrelated markdown edit sitting dirty in the worktree got swept into the commit alongside the intended changes. That was a **githook defect, not a `wiki` CLI defect** — the CLI behaved correctly; the hook's staging heuristic was just too broad.

The current hook snapshots the content hash of every tracked `.md` file *before* invoking `--fix`, then re-stages only the files whose hash differs afterward — precisely the set `--fix` actually rewrote, regardless of what else was dirty in the tree. If you're on an older copy of this hook (predating the hash-snapshot approach) and see unrelated `.md` changes appear in a commit, that's the blanket-sweep defect — replace the hook with [`./examples/pre-commit.wiki.sh`](../examples/pre-commit.wiki.sh).

## Why check the whole corpus, not just staged files

A staged edit can break a wikilink on an *unstaged* page or collide with an *untouched* page's title. Checking the entire corpus catches these cross-page failures and lets `--fix` repair them. The pass is idempotent — already-covered links are skipped — so it's cheap on every commit.

Preview before wiring it in: `wiki check --fix --fix-dry-run` shows created meshes and any planned slug-collision renames without mutating anything.

## Optional merge driver

The `.gitattributes` file includes `.wiki/** merge=wiki-mesh` — committed and shared by everyone — but the merge driver itself is a **per-clone** registration (see [Wiring it in](#wiring-it-in) above for the command). It used to be registered automatically on the hook's first run; that auto-registration was removed because a pre-commit hook is the wrong place to mutate clone-local git config on every commit; it's now a one-time manual step.

Registering it makes git call `wiki mesh merge` as the merge driver for `.wiki/` files when they conflict during a merge. It collapses easy conflicts mid-merge so they never leave markers in the first place.

**What it does:** When both sides touch disjoint anchors, or one side touches anchors while the other only changes the `--why` rationale, the driver combines them without conflict markers. When both sides touch the same anchor, markers remain — and `--fix` handles the rest.

**What it doesn't do:** The driver is a noise-reducing subset of `--fix`. It only resolves markers in `.wiki/` files; it doesn't re-hash anchors against the worktree, create coverage for new fragment links, or fix drifted links/anchors/frontmatter. Those are `--fix`'s job.

**Fallback:** Clones that skip setup or don't have the wiki CLI installed fall back to git's built-in line merge for `.wiki/` files. The result (conflict markers) is the same input `--fix` already handles — no loss of correctness, just slightly more noise during the merge.

### Why per-clone, not committed

Git doesn't let you commit merge driver configuration to `.git/config`. The `.gitattributes` line references a driver name, but the driver definition (`merge.wiki-mesh.driver`) lives in the clone-local config. The hook sets it once and it persists.
