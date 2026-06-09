# Git hook setup

One pre-commit concern: a single `wiki check --fix` call auto-repairs drifted links/anchors/frontmatter *and* creates mesh coverage for uncovered fragment links, then stages everything it touched — before the commit lands. Both error classes resolve in the same pass, so it's one invocation, not two.

It is a **local development guard, not a hard commit gate.** Errors `--fix` can't resolve (deleted target, ambiguous rename) print to stderr but never block the commit (`--no-exit-code`). If `wiki` isn't installed, the hook silently passes.

## The hook

`.githooks/pre-commit.wiki.sh`:

```bash
#!/bin/bash
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

# --fix rewrites in place (needs --source=worktree) and creates mesh coverage;
# --print-applied prints created/renamed mesh paths to stdout; --no-exit-code = advisory.
APPLIED=$("$WIKI_BIN" check --fix --print-applied --no-exit-code --source=worktree)

WIKI_FIXED=$(git diff --name-only --diff-filter=d -- '*.md')
if [ -n "$WIKI_FIXED" ]; then
    # shellcheck disable=SC2086
    git add $WIKI_FIXED
    echo "Re-staged wiki-fixed files:"; echo "$WIKI_FIXED"
fi

if [ -n "$APPLIED" ]; then
    while IFS= read -r mesh_path; do
        [ -n "$mesh_path" ] && git add -- "$mesh_path"
    done <<< "$APPLIED"
    echo "Staged scaffolded meshes:"; echo "$APPLIED"
fi
exit 0
```

## Why each flag is load-bearing

- **`--fix`** — rewrites drifted links/anchors/frontmatter **and** creates coverage for uncovered links. Requires `--source=worktree` (you can only rewrite files read from the worktree).
- **`--print-applied`** — prints one repo-relative path per mesh this run created or renamed to **stdout** (advisories/rename notices → stderr). The hook stages **exactly** those paths — never a blanket `git add .wiki/` — so unrelated working-tree `.wiki/` edits aren't swept in. After `--fix` rewrites `.md` files, the hook re-stages those too, so fixes land in the same commit.
- **`--no-exit-code`** — repair-and-continue instead of rejecting the commit.

## Why check the whole corpus, not just staged files

A staged edit can break a wikilink on an *unstaged* page or collide with an *untouched* page's title. Checking the entire corpus catches these cross-page failures and lets `--fix` repair them. The pass is idempotent — already-covered links are skipped — so it's cheap on every commit.

Preview before wiring it in: `wiki check --fix --fix-dry-run` shows created meshes and any planned slug-collision renames without mutating anything.
