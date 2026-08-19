# Git hook setup

One pre-commit concern: a single `wiki check --fix` call relocates drifted line-range links, routes broken targets through the rename machinery, and initializes `links-reviewed:` on field-less pages — then stages exactly the files that run rewrote, before the commit lands. Everything resolves in the same pass, so it's one invocation, not two.

It is a **local development guard, not a hard commit gate.** Errors `--fix` can't resolve (deleted target, ambiguous rename, in-place drift) print to stderr but never block the commit (`--no-exit-code`). If `wiki` isn't installed, the hook silently passes.

## The hook

`.githooks/pre-commit.wiki.sh`, reproduced verbatim at [`./examples/pre-commit.wiki.sh`](../examples/pre-commit.wiki.sh) so it can be copied straight into a repo:

```bash
#!/bin/bash
# Single wiki concern, single invocation:
#   wiki check --fix relocates drifted line-range links, routes broken targets
#   through the rename machinery, and initializes `links-reviewed:` on
#   field-less pages — all in the working tree.
# --no-exit-code makes this best-effort: the hook never aborts a commit.
# --print-applied prints the repo-relative path of each file the run rewrote
# to stdout (one per line); everything else goes to stderr (shown on the
# terminal).
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

# ── Single pass: auto-fix, then stage exactly what the run rewrote ────────────
# --fix rewrites in place (requires --source=worktree). --print-applied is the
# machine-readable list of rewritten files, so the hook stages precisely those
# paths — no snapshot/compare pass, and no sweeping in unrelated dirty .md
# edits the run did not touch.
APPLIED=$("$WIKI_BIN" check --fix --print-applied --no-exit-code --source=worktree)

if [ -n "$APPLIED" ]; then
    while IFS= read -r fixed_path; do
        [ -n "$fixed_path" ] && git add -- "$fixed_path"
    done <<< "$APPLIED"
    echo "Re-staged wiki-fixed files:"
    echo "$APPLIED"
fi
exit 0
```

## Wiring it in

One one-time, per-clone setup step — the hook itself does not do it automatically:

1. Copy [`./examples/pre-commit.wiki.sh`](../examples/pre-commit.wiki.sh) to `.githooks/pre-commit.wiki.sh` (or wherever your `core.hooksPath` points) and make it executable.

## Why each flag is load-bearing

- **`--fix`** — relocates drifted line-range links to where their certified content moved, routes broken targets through the rename machinery, and initializes `links-reviewed:` on field-less pages carrying line-range links. Requires `--source=worktree` (you can only rewrite files read from the worktree).
- **`--print-applied`** — prints one repo-relative path per file this run rewrote to **stdout** (the fix/skip summary and diagnostics go to stderr). The hook stages **exactly** those paths — never a blanket `git add` — so unrelated dirty edits in the working tree aren't swept in.
- **`--no-exit-code`** — repair-and-continue instead of rejecting the commit.

## Why staging is `--print-applied`, not a blanket `git diff`

An earlier version of this hook ran `--fix` and then staged `git diff --name-only` over every modified `.md` file — which is **not** scoped to files `--fix` touched; it's every modified `.md` file anywhere in the working tree at that moment, so unrelated markdown edits got swept into the commit. That was a **githook defect, not a `wiki` CLI defect**. A later version snapshotted content hashes of tracked `.md` files before the run and re-staged only the ones whose hash changed — correct, but heavier than it needed to be. `--print-applied` now reports *every* file the run rewrote (pages included, not just the old mesh paths), so the hook stages the machine-readable applied list directly and the snapshot pass is gone.

## Why check the whole corpus, not just staged files

A staged edit can break a wikilink on an *unstaged* page or collide with an *untouched* page's title. Checking the entire corpus catches these cross-page failures and lets `--fix` repair them. The pass is idempotent, so it's cheap on every commit.

Preview before wiring it in: `wiki check --fix --fix-dry-run` shows what would be rewritten without mutating anything.
