#!/bin/bash
# Auto-fix staged TS/JS/JSX with `biome check --fix`; re-stage fixes.
# Non-blocking.
set -e

command -v npx >/dev/null 2>&1 || exit 0

STAGED_FILES=$(git diff --cached --name-only --diff-filter=d)
BIOME_STAGED=$(echo "$STAGED_FILES" | grep -E '\.(ts|tsx|js|jsx)$' || true)
[ -n "$BIOME_STAGED" ] || exit 0

echo "Running biome check --fix on staged files..."
npx biome check --fix --staged --no-errors-on-unmatched

# --staged only filters which files to check; it does not re-add fixes to the
# index. Re-stage any staged file biome modified.
BIOME_CHANGED=""
for f in $BIOME_STAGED; do
    if ! git diff --quiet -- "$f" 2>/dev/null; then
        BIOME_CHANGED="$BIOME_CHANGED $f"
    fi
done
if [ -n "$BIOME_CHANGED" ]; then
    # shellcheck disable=SC2086
    git add $BIOME_CHANGED
    echo "Re-staged biome-fixed files:$BIOME_CHANGED"
fi
exit 0
