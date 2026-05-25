#!/bin/bash
# Single wiki concern, single invocation:
#   wiki check --fix creates/renames git meshes for uncovered fragment links and
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
