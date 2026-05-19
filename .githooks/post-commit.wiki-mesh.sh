#!/bin/bash
# Surface wiki fragment links lacking git mesh coverage and print the exact
# `git mesh add` / `git mesh why` commands for the committer to review.
# Advisory only — cannot and must not block the commit that already landed.
set -e

command -v wiki >/dev/null 2>&1 || exit 0
WIKI_BIN=$(command -v wiki)

WIKI_JSON=$("$WIKI_BIN" check --format json --no-exit-code 2>&1) || true

command -v jq >/dev/null 2>&1 || exit 0
[ -n "$WIKI_JSON" ] || exit 0

# Read one path per line into an array (mapfile -t) and pass it quoted to
# scaffold, so paths containing spaces survive as single args.
mapfile -t MESH_FILES < <(echo "$WIKI_JSON" \
    | jq -r '[.errors[] | select(.kind == "mesh_uncovered") | .file] | unique | .[]' \
    2>/dev/null || true)
if [ ${#MESH_FILES[@]} -gt 0 ]; then
    echo "Wiki fragment links lack mesh coverage. Review and run:"
    "$WIKI_BIN" scaffold "${MESH_FILES[@]}"
fi
exit 0
