#!/bin/bash
# Fail-closed: create git meshes for any fragment links lacking coverage,
# then stage the created .mesh/ files so they join the commit.
# Runs after pre-commit.wiki.sh (link/anchor auto-fix must precede coverage).
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

command -v jq >/dev/null 2>&1 || exit 0

# Capture stdout only; let stderr flow to the terminal.
# --no-exit-code means wiki check exits 0 even when validation errors exist,
# so a non-zero exit here means a real failure (crash/bad invocation) → fail closed.
WIKI_JSON=$("$WIKI_BIN" check --format json --no-exit-code)
[ -n "$WIKI_JSON" ] || exit 0

# Parse the JSON; if jq cannot parse it, that is an error condition → fail closed.
# shellcheck disable=SC2312
MESH_FILES_RAW=$(echo "$WIKI_JSON" \
    | jq -r '[.errors[] | select(.kind == "mesh_uncovered") | .file] | unique | .[]')

# Read one path per line into an array (mapfile -t) and pass it quoted to
# scaffold, so paths containing spaces survive as single args.
mapfile -t MESH_FILES <<< "$MESH_FILES_RAW"

# Filter out empty entries (jq emits nothing when there are no matches; mapfile
# then produces a one-element array containing the empty string).
NON_EMPTY=()
for f in "${MESH_FILES[@]}"; do
    [ -n "$f" ] && NON_EMPTY+=("$f")
done

if [ ${#NON_EMPTY[@]} -gt 0 ]; then
    "$WIKI_BIN" scaffold "${NON_EMPTY[@]}"
    # Only stage when the directory was actually created; absence of .mesh/ is
    # legitimate (e.g. all drafts dropped because a code target is missing).
    if [ -d .mesh ]; then
        git add .mesh/
    fi
fi
exit 0
