#!/usr/bin/env bash
set -o pipefail

# Detect whether the extension dist directory is a symlink (worktree environment).
# In a git worktree the dist/ directory is symlinked to the main repo's build output
# and vsce cannot follow symlinks when packaging — build only the CLI in that case.
EXTENSION_DIST="$(dirname "$0")/../packages/extension/dist"
if [ -L "$EXTENSION_DIST" ]; then
  echo "validate.sh: extension dist is a symlink (worktree); building CLI only — skipping vsce packaging; tsc/lint still validated extension" >&2
  BUILD_CMD="yarn workspace @goodfoot/wiki build"
else
  BUILD_CMD="SKIP_INSTALL=1 yarn build"
fi

# `wiki check` must run the CLI built from THIS tree, not whatever stale `wiki`
# is on PATH (e.g. the VS Code extension's installed binary), or it will miss
# fixes in this commit. Build the CLI release up front, then prepend its
# directory to PATH. Binary name and target dir are platform/script specific:
# `yarn build` uses CARGO_TARGET_DIR=target/build, and Windows produces
# `wiki.exe`.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
case "$(uname -s 2>/dev/null || echo unknown)" in
  MINGW* | MSYS* | CYGWIN* | Windows_NT) BIN_NAME="wiki.exe" ;;
  *) BIN_NAME="wiki" ;;
esac

echo "validate.sh: building CLI release so wiki check uses this tree's binary..." >&2
if ! yarn workspace @goodfoot/wiki build; then
  echo "validate.sh: CLI release build failed" >&2
  exit 1
fi

LOCAL_WIKI="$SCRIPT_DIR/../packages/cli/target/build/release/$BIN_NAME"
if [ -x "$LOCAL_WIKI" ]; then
  export PATH="$(dirname "$LOCAL_WIKI"):$PATH"
  echo "validate.sh: using $("$LOCAL_WIKI" --version 2>/dev/null || echo "$LOCAL_WIKI")" >&2
else
  echo "validate.sh: expected built binary not found at $LOCAL_WIKI" >&2
  exit 1
fi

# Fail closed when rebuilding the agent-hooks bundles dirties the committed
# plugin trees: a stale or hand-edited bundle must never ship silently.
agent_hooks_bundles_fresh() {
  if [ -n "$(git status --porcelain -- plugins-claude/wiki/hooks plugins-codex/wiki/hooks plugins-opencode/wiki/dist)" ]; then
    echo "ERROR: rebuild produced uncommitted bundle changes — commit the rebuilt plugin bundles" >&2
    return 1
  fi
}

{
  yarn typecheck &&
  yarn lint &&
  "$LOCAL_WIKI" check &&
  yarn workspace @goodfoot/agent-hooks build &&
  agent_hooks_bundles_fresh &&
  yarn test &&
  eval "$BUILD_CMD"
} 2>&1 | tee yarn-validate-output.log

EXIT_CODE=${PIPESTATUS[0]}
echo "Exit code: $EXIT_CODE" | tee -a yarn-validate-output.log
exit $EXIT_CODE
