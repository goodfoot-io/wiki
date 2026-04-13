#!/usr/bin/env bash
set -eo pipefail

# Wiki Gap Detection (Gemini)
#
# Scans recent commits for source files that have no wiki fragment link coverage.
# For each uncovered file that passes an inclusion gate, Gemini creates or expands
# wiki pages in an isolated worktree, then merges changes back.
#
# Configurable variables:
#   WIKI_DIR   — wiki directory relative to repo root (default: "wiki")
#   LOG_DIR    — log directory relative to repo root (default: ".wiki/logs")

# Prevent infinite recursion from post-commit hook
if [ -n "$GEMINI_WIKI_ACTIVE" ]; then
  exit 0
fi
export GEMINI_WIKI_ACTIVE=1

REPO_ROOT=$(git rev-parse --show-toplevel)
GIT_COMMON_DIR=$(git rev-parse --git-common-dir)
cd "$REPO_ROOT"

WIKI_DIR="${WIKI_DIR:-wiki}"
LOG_DIR="${LOG_DIR:-.wiki/logs}"
LOG_FILE="$REPO_ROOT/$LOG_DIR/gemini-wiki-gap-detection.log"
STATE_FILE="$GIT_COMMON_DIR/wiki-gap-detection.last-sha"

mkdir -p "$(dirname "$LOG_FILE")"
exec > >(tee -a "$LOG_FILE") 2>&1
echo "--- $(date '+%Y-%m-%d %H:%M:%S') wiki gap detection started (PID $$) ---"

# Determine commit range to scan
CURRENT_SHA=$(git rev-parse HEAD)

if [ -f "$STATE_FILE" ]; then
  SINCE_SHA=$(cat "$STATE_FILE")
  if ! git rev-parse --quiet --verify "$SINCE_SHA^{commit}" > /dev/null 2>&1; then
    echo "Stored SHA $SINCE_SHA no longer valid; scanning last 20 commits."
    SINCE_SHA=$(git rev-list --max-count=20 HEAD | tail -1)
  fi
else
  echo "No previous run recorded; scanning last 10 commits."
  SINCE_SHA=$(git rev-list --max-count=10 HEAD | tail -1)
fi

if [ "$SINCE_SHA" = "$CURRENT_SHA" ]; then
  echo "No new commits since last run. Exiting."
  exit 0
fi

# Find source files added or modified since the last run (exclude wiki, tests, generated files)
CHANGED_FILES=$(git log --name-only --diff-filter=AM --format='' "$SINCE_SHA".."$CURRENT_SHA" \
  -- 'packages/**/*.rs' 'packages/**/*.ts' \
  | sort -u \
  | grep -v -E '(\.wiki\.md$|\.test\.|\.spec\.|__tests__|/tests?/)' \
  || true)

if [ -z "$CHANGED_FILES" ]; then
  echo "No source files changed in range. Exiting."
  echo "$CURRENT_SHA" > "$STATE_FILE"
  exit 0
fi

# Find source files with no wiki fragment link coverage
UNCOVERED_FILES=""
while IFS= read -r file; do
  [ -z "$file" ] && continue
  [ ! -f "$REPO_ROOT/$file" ] && continue
  if ! wiki refs "$file" 2>/dev/null | grep -q .; then
    UNCOVERED_FILES=$(printf '%s\n%s' "$UNCOVERED_FILES" "$file")
  fi
done <<< "$CHANGED_FILES"

UNCOVERED_FILES=$(echo "$UNCOVERED_FILES" | grep -v '^$' || true)

if [ -z "$UNCOVERED_FILES" ]; then
  echo "All changed source files have wiki coverage. Exiting."
  echo "$CURRENT_SHA" > "$STATE_FILE"
  exit 0
fi

echo "Uncovered files to assess:"
echo "$UNCOVERED_FILES"

# Fail-closed single-flight lockfile
LOCKDIR="$GIT_COMMON_DIR/wiki-gap-detection.lock"
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
  echo "Wiki gap detection is already running."
  exit 0
fi
echo $$ > "$PIDFILE"

WORKTREE_DIR=""
ISOLATED_HOME=""
GEMINI_OUTPUT=""

cleanup() {
  local exit_code=$?
  if [ "$exit_code" -ne 0 ]; then
    echo "--- $(date '+%Y-%m-%d %H:%M:%S') wiki gap detection failed (exit $exit_code) ---" >&2
  else
    echo "--- $(date '+%Y-%m-%d %H:%M:%S') wiki gap detection complete ---"
  fi
  rm -rf "$LOCKDIR"
  if [ -n "$WORKTREE_DIR" ] && [ -d "$WORKTREE_DIR" ]; then
    git worktree remove --force "$WORKTREE_DIR" 2>/dev/null || true
  fi
  if [ -n "$TEMP_BRANCH" ]; then
    git branch -D "$TEMP_BRANCH" 2>/dev/null || true
  fi
  [ -n "$ISOLATED_HOME" ] && [ -d "$ISOLATED_HOME" ] && rm -rf "$ISOLATED_HOME"
  [ -n "$GEMINI_OUTPUT" ] && [ -f "$GEMINI_OUTPUT" ] && rm -f "$GEMINI_OUTPUT"
}
trap cleanup EXIT

START_HEAD=$(git rev-parse HEAD)
TEMP_BRANCH="wiki-gap-detection/$(date +%s)"

# Create isolated worktree
WORKTREE_DIR=$(mktemp -d)
git worktree add -b "$TEMP_BRANCH" "$WORKTREE_DIR" "$START_HEAD"

ISOLATED_HOME=$(mktemp -d)
GEMINI_OUTPUT=$(mktemp)
COMMIT_MSG_FILE="$WORKTREE_DIR/gemini-commit-message.txt"

if [ -d "$HOME/.gemini" ]; then
  cp -r "$HOME/.gemini" "$ISOLATED_HOME/.gemini"
fi

# Build uncovered file list for the prompt
UNCOVERED_LIST=""
while IFS= read -r file; do
  [ -z "$file" ] && continue
  UNCOVERED_LIST="$UNCOVERED_LIST- \`$file\`
"
done <<< "$UNCOVERED_FILES"

echo "Starting Gemini Wiki Gap Detection..."

if ! (cd "$WORKTREE_DIR" && HOME="$ISOLATED_HOME" PATH="$PATH" timeout 900 gemini \
  --model gemini-2.5-flash \
  --prompt "
## Wiki Gap Detection

You are performing wiki documentation gap analysis. The following source files were recently added or modified and have no wiki page referencing them via fragment links.

Commit range: \`$SINCE_SHA..$CURRENT_SHA\`

### Inclusion Gate

A file must pass ALL three criteria:
1. Content can be anchored to source code with fragment links
2. Content synthesizes across files, packages, or bounded contexts
3. Content answers 'why', 'how it connects', or 'what role it plays'

### Uncovered Files

$UNCOVERED_LIST

### Instructions

1. Read each file and apply the inclusion gate
2. Search for existing wiki pages before creating new ones: \`wiki \"<concept>\"\`
3. Create or expand pages with fragment links to definitions
4. Run \`wiki check --fix\` then \`wiki stale\` then \`wiki check\` — all must be clean
5. Write commit message to \`$COMMIT_MSG_FILE\` using write_file

Format: wiki: document [summary]
" | tee "$GEMINI_OUTPUT"); then
  echo "Gemini exited with error — gap detection aborted" >&2
  exit 1
fi

# Post-run verification
if ! (cd "$WORKTREE_DIR" && wiki check); then
  echo "Wiki check failed after agent run. Aborting." >&2
  exit 1
fi

# Check if any wiki files were created or modified
WORKTREE_WIKI_FILES=$(cd "$WORKTREE_DIR" && {
  git diff --name-only HEAD
  git ls-files --others --exclude-standard -- "$WIKI_DIR/" '*.wiki.md'
} | grep -E "^${WIKI_DIR}/|.*\.wiki\.md$" | grep -v 'gemini-commit-message\.txt' | sort -u || true)

if [ -z "$WORKTREE_WIKI_FILES" ]; then
  echo "No wiki files created or modified. Exiting."
  echo "$CURRENT_SHA" > "$STATE_FILE"
  exit 0
fi

# Determine commit message
if [ -f "$COMMIT_MSG_FILE" ] && [ -s "$COMMIT_MSG_FILE" ]; then
  FINAL_COMMIT_MSG_FILE="$COMMIT_MSG_FILE"
else
  FINAL_COMMIT_MSG_FILE=$(mktemp)
  echo "wiki: gap detection — automated documentation" > "$FINAL_COMMIT_MSG_FILE"
fi

# Stage and commit wiki changes in the worktree
(cd "$WORKTREE_DIR" && \
  for file in $WORKTREE_WIKI_FILES; do
    git add "$file"
  done && \
  git commit --file "$FINAL_COMMIT_MSG_FILE")

# Two-path merge: clean merge or patch apply
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
    echo "Wiki gap detection committed."
  else
    echo "Wiki update conflicted with local changes. Will retry on next run." >&2
    # shellcheck disable=SC2086
    git checkout HEAD -- $PATCH_FILES 2>/dev/null || true
  fi
fi

git rev-parse HEAD > "$STATE_FILE"
