#!/bin/bash
# Fail-closed: create git meshes for any fragment links lacking coverage,
# then stage the created .mesh/ files so they join the commit.
# Runs after pre-commit.wiki.sh (link/anchor auto-fix must precede coverage).
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

command -v jq >/dev/null 2>&1 || exit 0

WIKI_JSON=$("$WIKI_BIN" check --format json --no-exit-code 2>&1) || true
[ -n "$WIKI_JSON" ] || exit 0

# Read one path per line into an array (mapfile -t) and pass it quoted to
# scaffold, so paths containing spaces survive as single args.
mapfile -t MESH_FILES < <(echo "$WIKI_JSON" \
    | jq -r '[.errors[] | select(.kind == "mesh_uncovered") | .file] | unique | .[]' \
    2>/dev/null || true)
if [ ${#MESH_FILES[@]} -gt 0 ]; then
    "$WIKI_BIN" scaffold "${MESH_FILES[@]}"
    git add .mesh/
fi
exit 0
