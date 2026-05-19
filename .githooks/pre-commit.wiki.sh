#!/bin/bash
# Auto-fix wiki links/frontmatter on staged .md files; re-stage fixes.
# Non-blocking (--no-exit-code); mesh coverage is enforced by pre-commit.wiki-mesh.sh.
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

# --fix rewrites drifted links/anchors in place (requires --source=worktree).
"$WIKI_BIN" check --fix --no-exit-code --no-mesh --source=worktree

WIKI_FIXED=$(git diff --name-only --diff-filter=d -- '*.md')
if [ -n "$WIKI_FIXED" ]; then
    # shellcheck disable=SC2086
    git add $WIKI_FIXED
    echo "Re-staged wiki-fixed files:"
    echo "$WIKI_FIXED"
fi
exit 0
