#!/usr/bin/env bash
set -o pipefail

# Detect whether the extension dist directory is a symlink (worktree environment).
# In a git worktree the dist/ directory is symlinked to the main repo's build output
# and vsce cannot follow symlinks when packaging — build only the CLI in that case.
EXTENSION_DIST="$(dirname "$0")/../packages/extension/dist"
if [ -L "$EXTENSION_DIST" ]; then
  BUILD_CMD="yarn workspace @goodfoot/wiki build"
else
  BUILD_CMD="SKIP_INSTALL=1 yarn build"
fi

{
  yarn typecheck &&
  yarn lint &&
  wiki check &&
  yarn test &&
  eval "$BUILD_CMD"
} 2>&1 | tee yarn-validate-output.log

EXIT_CODE=${PIPESTATUS[0]}
echo "Exit code: $EXIT_CODE" | tee -a yarn-validate-output.log
exit $EXIT_CODE
