#!/bin/bash
# Single wiki concern, single invocation:
#   wiki check --fix relocates drifted line-range links, routes broken targets
#   through the rename machinery, and initializes `links-reviewed:` on
#   field-less pages — all in the working tree.
# --no-exit-code makes this best-effort: the hook never aborts a commit.
# --print-applied prints the repo-relative path of each file the run rewrote
# to stdout (one per line); everything else goes to stderr.
set -e

WIKI_BIN="${WIKI_BIN:-$(command -v wiki 2>/dev/null || true)}"
[ -n "$WIKI_BIN" ] && [ -x "$WIKI_BIN" ] || exit 0

APPLIED=$("$WIKI_BIN" check --fix --print-applied --no-exit-code --source=worktree) || exit 0

if [ -n "$APPLIED" ]; then
    while IFS= read -r fixed_path; do
        [ -n "$fixed_path" ] && git add -- "$fixed_path"
    done <<< "$APPLIED"
    echo "Re-staged wiki-fixed files:"
    echo "$APPLIED"
fi
exit 0
