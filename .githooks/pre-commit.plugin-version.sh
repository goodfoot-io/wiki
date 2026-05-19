#!/bin/bash
# Auto-bump patch version of any changed plugin's plugin.json, sync that
# version into marketplace.json, and bump marketplace.json's own version.
# Re-stages everything it rewrites. Non-blocking (consistency is gated
# separately in pre-commit.version-consistency.sh).
set -e

STAGED_FILES=$(git diff --cached --name-only --diff-filter=d)
[ -n "$STAGED_FILES" ] || exit 0

# Track which plugins have been processed to avoid double-bumping
declare -A PROCESSED_PLUGINS
PLUGINS_BUMPED=0

for file in $STAGED_FILES; do
    if [[ "$file" =~ ^plugins/([^/]+)/ ]]; then
        PLUGIN_NAME="${BASH_REMATCH[1]}"
        PLUGIN_JSON="plugins/${PLUGIN_NAME}/.claude-plugin/plugin.json"

        if [[ -n "${PROCESSED_PLUGINS[$PLUGIN_NAME]}" ]]; then
            continue
        fi

        # Skip if the only change is to plugin.json itself (avoid infinite loops)
        PLUGIN_STAGED_FILES=$(echo "$STAGED_FILES" | grep "^plugins/${PLUGIN_NAME}/" || true)
        NON_PLUGIN_JSON_FILES=$(echo "$PLUGIN_STAGED_FILES" | grep -v "\.claude-plugin/plugin.json$" || true)

        if [ -z "$NON_PLUGIN_JSON_FILES" ]; then
            PROCESSED_PLUGINS[$PLUGIN_NAME]=1
            continue
        fi

        if [ ! -f "$PLUGIN_JSON" ]; then
            echo "Warning: $PLUGIN_JSON not found, skipping version bump for $PLUGIN_NAME"
            PROCESSED_PLUGINS[$PLUGIN_NAME]=1
            continue
        fi

        CURRENT_VERSION=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$PLUGIN_JSON" | grep -o '[0-9]\+\.[0-9]\+\.[0-9]\+')

        if [ -z "$CURRENT_VERSION" ]; then
            echo "Warning: Could not parse version from $PLUGIN_JSON, skipping"
            PROCESSED_PLUGINS[$PLUGIN_NAME]=1
            continue
        fi

        IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"
        NEW_PATCH=$((PATCH + 1))
        NEW_VERSION="${MAJOR}.${MINOR}.${NEW_PATCH}"

        if [[ "$OSTYPE" == "darwin"* ]]; then
            sed -i '' "s/\"version\"[[:space:]]*:[[:space:]]*\"${CURRENT_VERSION}\"/\"version\": \"${NEW_VERSION}\"/" "$PLUGIN_JSON"
        else
            sed -i "s/\"version\"[[:space:]]*:[[:space:]]*\"${CURRENT_VERSION}\"/\"version\": \"${NEW_VERSION}\"/" "$PLUGIN_JSON"
        fi

        git add "$PLUGIN_JSON"
        echo "Bumped ${PLUGIN_NAME} version: ${CURRENT_VERSION} -> ${NEW_VERSION}"

        MARKETPLACE_JSON=".claude-plugin/marketplace.json"
        if [ -f "$MARKETPLACE_JSON" ] && command -v jq &> /dev/null; then
            jq --arg name "$PLUGIN_NAME" --arg version "$NEW_VERSION" \
                '(.plugins[] | select(.name == $name)).version = $version' \
                "$MARKETPLACE_JSON" > "${MARKETPLACE_JSON}.tmp" && \
                mv "${MARKETPLACE_JSON}.tmp" "$MARKETPLACE_JSON"
            echo "Synced ${PLUGIN_NAME} version in marketplace.json: ${NEW_VERSION}"
        fi

        PROCESSED_PLUGINS[$PLUGIN_NAME]=1
        PLUGINS_BUMPED=1
    fi
done

# If any plugins were bumped, also bump the marketplace.json metadata.version
if [ "$PLUGINS_BUMPED" -eq 1 ]; then
    MARKETPLACE_JSON=".claude-plugin/marketplace.json"

    if [ -f "$MARKETPLACE_JSON" ]; then
        MARKETPLACE_VERSION=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$MARKETPLACE_JSON" | head -1 | grep -o '[0-9]\+\.[0-9]\+\.[0-9]\+')

        if [ -n "$MARKETPLACE_VERSION" ]; then
            IFS='.' read -r M_MAJOR M_MINOR M_PATCH <<< "$MARKETPLACE_VERSION"
            NEW_M_PATCH=$((M_PATCH + 1))
            NEW_MARKETPLACE_VERSION="${M_MAJOR}.${M_MINOR}.${NEW_M_PATCH}"

            if [[ "$OSTYPE" == "darwin"* ]]; then
                sed -i '' "0,/\"version\"[[:space:]]*:[[:space:]]*\"${MARKETPLACE_VERSION}\"/s//\"version\": \"${NEW_MARKETPLACE_VERSION}\"/" "$MARKETPLACE_JSON"
            else
                sed -i "0,/\"version\"[[:space:]]*:[[:space:]]*\"${MARKETPLACE_VERSION}\"/s//\"version\": \"${NEW_MARKETPLACE_VERSION}\"/" "$MARKETPLACE_JSON"
            fi

            git add "$MARKETPLACE_JSON"
            echo "Bumped marketplace.json version: ${MARKETPLACE_VERSION} -> ${NEW_MARKETPLACE_VERSION}"
        fi
    fi
fi

exit 0
