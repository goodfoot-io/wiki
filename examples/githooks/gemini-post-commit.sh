#!/usr/bin/env bash
# post-commit hook: Gemini wiki gap detection + maintenance
# Install: copy to .githooks/post-commit and run git config core.hooksPath .githooks

# Prevent recursion — do not run during our own wiki commits
if [ -n "$GEMINI_WIKI_ACTIVE" ]; then
  exit 0
fi

REPO_ROOT=$(git rev-parse --show-toplevel)
SCRIPTS_DIR="$REPO_ROOT/examples/githooks/scripts"

# Unset GIT env vars that interfere with nested git operations
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES

# Run gap detection in background
if [ -x "$SCRIPTS_DIR/gemini-wiki-gap-detection.sh" ]; then
  nohup "$SCRIPTS_DIR/gemini-wiki-gap-detection.sh" > /dev/null 2>&1 &
fi

# Run maintenance in background
if [ -x "$SCRIPTS_DIR/gemini-wiki-maintenance.sh" ]; then
  nohup "$SCRIPTS_DIR/gemini-wiki-maintenance.sh" > /dev/null 2>&1 &
fi

exit 0
