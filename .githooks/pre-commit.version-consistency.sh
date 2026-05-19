#!/bin/bash
# Gate: every plugin's version in marketplace.json must match its own
# plugin.json. Fail-closed — a mismatch aborts the commit.
set -e

command -v jq >/dev/null 2>&1 || exit 0
MARKETPLACE_JSON=".claude-plugin/marketplace.json"
[ -f "$MARKETPLACE_JSON" ] || exit 0

VALIDATION_FAILED=0
while IFS=$'\t' read -r PLUGIN_NAME MARKETPLACE_VER; do
    PLUGIN_JSON="plugins/${PLUGIN_NAME}/.claude-plugin/plugin.json"
    if [ -f "$PLUGIN_JSON" ]; then
        PLUGIN_VER=$(jq -r '.version' "$PLUGIN_JSON")
        if [ "$MARKETPLACE_VER" != "$PLUGIN_VER" ]; then
            echo "ERROR: Version mismatch for ${PLUGIN_NAME} plugin:"
            echo "  marketplace.json: $MARKETPLACE_VER"
            echo "  plugin.json: $PLUGIN_VER"
            VALIDATION_FAILED=1
        fi
    fi
done < <(jq -r '.plugins[] | [.name, .version] | @tsv' "$MARKETPLACE_JSON")

if [ "$VALIDATION_FAILED" -eq 1 ]; then
    echo ""
    echo "Commit blocked: Plugin versions in marketplace.json must match their plugin.json versions."
    exit 1
fi
exit 0
