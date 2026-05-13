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

# Prefer the locally-built wiki binary so that wiki check uses the same version
# of the CLI that the repo builds (avoids ancestor-walk behaviour in older installs).
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LOCAL_WIKI="$SCRIPT_DIR/../packages/cli/target/release/wiki"
if [ -x "$LOCAL_WIKI" ]; then
  export PATH="$(dirname "$LOCAL_WIKI"):$PATH"
fi

{
  yarn typecheck &&
  yarn lint &&
  wiki check --root wiki &&
  yarn test &&
  eval "$BUILD_CMD"
} 2>&1 | tee yarn-validate-output.log

EXIT_CODE=${PIPESTATUS[0]}
echo "Exit code: $EXIT_CODE" | tee -a yarn-validate-output.log
exit $EXIT_CODE
