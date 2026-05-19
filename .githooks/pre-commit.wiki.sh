#!/bin/bash
# Single wiki concern, two phases:
#   1. Auto-fix drifted wiki links/anchors/frontmatter on the working tree and
#      re-stage the fixed .md files (non-blocking — `--no-exit-code`).
#   2. Create git mesh coverage for any uncovered fragment links and stage
#      exactly the meshes this run created/renamed (fail-closed).
# Phase 2 needs `git-mesh`; a missing binary or a real scaffold failure aborts
# the commit. `wiki scaffold` self-discovers uncovered links and is idempotent,
# so no separate `wiki check`/jq pre-filter is needed.
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

# ── Phase 1: auto-fix links/anchors/frontmatter, re-stage ────────────────────
# --fix rewrites in place (requires --source=worktree); --no-mesh keeps mesh
# coverage out of this phase (phase 2 owns it); --no-exit-code = advisory.
"$WIKI_BIN" check --fix --no-exit-code --no-mesh --source=worktree

WIKI_FIXED=$(git diff --name-only --diff-filter=d -- '*.md')
if [ -n "$WIKI_FIXED" ]; then
    # shellcheck disable=SC2086
    git add $WIKI_FIXED
    echo "Re-staged wiki-fixed files:"
    echo "$WIKI_FIXED"
fi

# ── Phase 2: mesh coverage (fail-closed) ─────────────────────────────────────
# --print-applied: stdout = one repo-relative path per created/renamed mesh;
# advisories and rename notices go to stderr (shown on the terminal). A
# non-zero exit (git-mesh unavailable, or a genuine `git mesh add` failure)
# fails the commit closed.
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
