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
