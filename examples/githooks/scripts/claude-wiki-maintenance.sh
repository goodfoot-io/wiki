#!/usr/bin/env bash
set -eo pipefail

# Wiki Maintenance (Claude)
#
# Detects stale wiki fragment links and uses Claude to update prose,
# re-pin links, and maintain wiki coherence in an isolated worktree.
#
# Configurable variables:
#   WIKI_DIR   — wiki directory relative to repo root (default: "wiki")
#   LOG_DIR    — log directory relative to repo root (default: ".wiki/logs")

# Prevent infinite recursion from post-commit hook
if [ -n "$CLAUDE_WIKI_ACTIVE" ]; then
  exit 0
fi
export CLAUDE_WIKI_ACTIVE=1

REPO_ROOT=$(git rev-parse --show-toplevel)
GIT_COMMON_DIR=$(git rev-parse --git-common-dir)
cd "$REPO_ROOT"

WIKI_DIR="${WIKI_DIR:-wiki}"
LOG_DIR="${LOG_DIR:-.wiki/logs}"
LOG_FILE="$REPO_ROOT/$LOG_DIR/claude-wiki-maintenance.log"

mkdir -p "$(dirname "$LOG_FILE")"
exec > >(tee -a "$LOG_FILE") 2>&1
echo "--- $(date '+%Y-%m-%d %H:%M:%S') wiki maintenance started (PID $$) ---"

# Exit early if there are no stale links
if wiki stale > /dev/null 2>&1; then
  echo "No stale wiki links. Exiting."
  exit 0
fi

# Fail-closed single-flight lockfile
LOCKDIR="$GIT_COMMON_DIR/wiki-maintenance.lock"
PIDFILE="$LOCKDIR/pid"

acquire_lock() {
  if ! mkdir "$LOCKDIR" 2>/dev/null; then
    if [ -f "$PIDFILE" ]; then
      local lock_pid
      lock_pid=$(cat "$PIDFILE")
      if ! kill -0 "$lock_pid" 2>/dev/null; then
        echo "Found stale lock for PID $lock_pid. Removing..."
        rm -rf "$LOCKDIR"
        mkdir "$LOCKDIR" || return 1
        return 0
      fi
    fi
    return 1
  fi
  return 0
}

if ! acquire_lock; then
  echo "Wiki maintenance is already running."
  exit 0
fi
echo $$ > "$PIDFILE"

WORKTREE_DIR=""

cleanup() {
  local exit_code=$?
  if [ "$exit_code" -ne 0 ]; then
    echo "--- $(date '+%Y-%m-%d %H:%M:%S') wiki maintenance failed (exit $exit_code) ---" >&2
  else
    echo "--- $(date '+%Y-%m-%d %H:%M:%S') wiki maintenance complete ---"
  fi
  rm -rf "$LOCKDIR"
  if [ -n "$WORKTREE_DIR" ] && [ -d "$WORKTREE_DIR" ]; then
    git worktree remove --force "$WORKTREE_DIR" 2>/dev/null || true
  fi
  if [ -n "$TEMP_BRANCH" ]; then
    git branch -D "$TEMP_BRANCH" 2>/dev/null || true
  fi
}
trap cleanup EXIT

START_HEAD=$(git rev-parse HEAD)
TEMP_BRANCH="wiki-maintenance/$(date +%s)"

# Create isolated worktree
WORKTREE_DIR=$(mktemp -d)
git worktree add -b "$TEMP_BRANCH" "$WORKTREE_DIR" "$START_HEAD"

COMMIT_MSG_FILE="$WORKTREE_DIR/claude-commit-message.txt"

echo "Starting Claude Wiki Maintenance..."

PROMPT="## Wiki Maintenance

### Step 1: Identify Stale Pages

\`\`\`bash
wiki stale
\`\`\`

If no stale links, run \`wiki check --fix\`, write commit message, and stop.

### Step 2: For Each Stale Page

1. Get the diff: \`wiki stale --diff patch 'path/to/page.md'\`
2. Classify each change: behavior changed, boundary changed, cosmetic only, or code deleted
3. Update prose to reflect current code state
4. Apply fragment link discipline — if you mention it, link it
5. Re-pin: \`wiki pin\`
6. Check backlinks: \`wiki backlinks \"Page Title\"\`

### Step 3: Verify

\`\`\`bash
wiki check --fix
wiki stale
wiki check
\`\`\`

All must be clean.

### Step 4: Commit Message

Write to \`$COMMIT_MSG_FILE\`. Format:

wiki: maintenance pass — [summary]

- [page]: [what changed]
- [page]: re-pinned only

Do not commit changes yourself."

if ! (cd "$WORKTREE_DIR" && timeout 900 claude --print --max-turns 30 "$PROMPT"); then
  echo "Claude exited with error — wiki maintenance aborted" >&2
  exit 1
fi

# Post-run verification
if ! (cd "$WORKTREE_DIR" && wiki check); then
  echo "Wiki check failed after agent run. Aborting." >&2
  exit 1
fi

# Check for wiki changes
WORKTREE_WIKI_FILES=$(cd "$WORKTREE_DIR" && git diff --name-only HEAD \
  | grep -E "^${WIKI_DIR}/|.*\.wiki\.md$" \
  | grep -v 'claude-commit-message\.txt' || true)

if [ -z "$WORKTREE_WIKI_FILES" ]; then
  echo "No wiki files modified. Exiting."
  exit 0
fi

# Determine commit message
if [ -f "$COMMIT_MSG_FILE" ] && [ -s "$COMMIT_MSG_FILE" ]; then
  FINAL_COMMIT_MSG_FILE="$COMMIT_MSG_FILE"
else
  FINAL_COMMIT_MSG_FILE=$(mktemp)
  echo "wiki: maintenance pass — automated wiki maintenance" > "$FINAL_COMMIT_MSG_FILE"
fi

# Stage and commit in worktree
(cd "$WORKTREE_DIR" && \
  for file in $WORKTREE_WIKI_FILES; do
    git add "$file"
  done && \
  git commit --file "$FINAL_COMMIT_MSG_FILE")

# Two-path merge
PATCH=$(git diff "$START_HEAD".."$TEMP_BRANCH" -- $(echo "$WORKTREE_WIKI_FILES" | tr '\n' ' '))

if git diff --quiet && git diff --cached --quiet; then
  echo "Working directory clean. Merging wiki updates..."
  git merge "$TEMP_BRANCH"
else
  echo "Working directory has uncommitted changes. Applying wiki updates..."
  PATCH_FILES=$(echo "$WORKTREE_WIKI_FILES" | tr '\n' ' ')
  if echo "$PATCH" | git apply -; then
    for file in $PATCH_FILES; do
      git add "$file"
    done
    git commit --file "$FINAL_COMMIT_MSG_FILE"
    echo "Wiki maintenance committed."
  else
    echo "Wiki update conflicted with local changes. Will retry on next run." >&2
    # shellcheck disable=SC2086
    git checkout HEAD -- $PATCH_FILES 2>/dev/null || true
  fi
fi
