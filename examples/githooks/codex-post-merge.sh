#!/usr/bin/env bash
# post-merge hook: Codex wiki maintenance after merge
# Install: copy to .githooks/post-merge and run git config core.hooksPath .githooks

# Prevent recursion
if [ -n "$CODEX_WIKI_ACTIVE" ]; then
  exit 0
fi

REPO_ROOT=$(git rev-parse --show-toplevel)
SCRIPTS_DIR="$REPO_ROOT/examples/githooks/scripts"

# Unset GIT env vars that interfere with nested git operations
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES

# Run maintenance in background
if [ -x "$SCRIPTS_DIR/codex-wiki-maintenance.sh" ]; then
  nohup "$SCRIPTS_DIR/codex-wiki-maintenance.sh" > /dev/null 2>&1 &
fi

exit 0
