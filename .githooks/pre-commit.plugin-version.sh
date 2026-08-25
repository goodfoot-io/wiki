#!/bin/bash
# Auto-bump patch version of any changed plugin's manifests, keeping every
# platform manifest of that plugin at one shared version, sync that version
# into marketplace.json, and bump marketplace.json's own version.
# Re-stages everything it rewrites. Non-blocking (consistency is gated
# separately in pre-commit.version-consistency.sh).
set -e

STAGED_FILES=$(git diff --cached --name-only --diff-filter=d)
[ -n "$STAGED_FILES" ] || exit 0

# Plugin trees: plugins-<platform>/<name>/ with a per-platform manifest:
#   plugins-claude/<name>/.claude-plugin/plugin.json
#   plugins-codex/<name>/.codex-plugin/plugin.json
#   plugins-opencode/<name>/package.json
manifest_paths_for() {
    local PLUGIN_NAME="$1"
    printf '%s\n' \
        "plugins-claude/${PLUGIN_NAME}/.claude-plugin/plugin.json" \
        "plugins-codex/${PLUGIN_NAME}/.codex-plugin/plugin.json" \
        "plugins-opencode/${PLUGIN_NAME}/package.json"
}

# Collect unique plugin names touched by this commit.
declare -A TOUCHED_PLUGINS
while IFS= read -r file; do
    if [[ "$file" =~ ^plugins-(claude|codex|opencode)/([^/]+)/ ]]; then
        TOUCHED_PLUGINS["${BASH_REMATCH[2]}"]=1
    fi
done <<< "$STAGED_FILES"

[ "${#TOUCHED_PLUGINS[@]}" -gt 0 ] || exit 0

PLUGINS_BUMPED=0

for PLUGIN_NAME in "${!TOUCHED_PLUGINS[@]}"; do
    # Skip if the only staged changes for this plugin are its own manifests
    # (avoid re-bumping when only versions change).
    PLUGIN_STAGED_FILES=$(echo "$STAGED_FILES" | grep -E "^plugins-(claude|codex|opencode)/${PLUGIN_NAME}/" || true)
    NON_MANIFEST_FILES=""
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        skip=0
        while IFS= read -r m; do
            if [ "$f" = "$m" ]; then
                skip=1
                break
            fi
        done < <(manifest_paths_for "$PLUGIN_NAME")
        if [ "$skip" -eq 0 ]; then
            NON_MANIFEST_FILES+="${f}"$'\n'
        fi
    done <<< "$PLUGIN_STAGED_FILES"

    if [ -z "$NON_MANIFEST_FILES" ]; then
        continue
    fi

    # Every platform manifest of this plugin moves to ONE shared new version.
    EXISTING_MANIFESTS=()
    REFERENCE_MANIFEST=""
    while IFS= read -r m; do
        if [ -f "$m" ]; then
            EXISTING_MANIFESTS+=("$m")
            [ -z "$REFERENCE_MANIFEST" ] && REFERENCE_MANIFEST="$m"
        fi
    done < <(manifest_paths_for "$PLUGIN_NAME")

    if [ "${#EXISTING_MANIFESTS[@]}" -eq 0 ]; then
        echo "ERROR: staged changes under the ${PLUGIN_NAME} plugin trees but no plugin manifest found (expected one of:" >&2
        manifest_paths_for "$PLUGIN_NAME" | sed 's/^/ERROR:   /' >&2
        echo "ERROR:)" >&2
        exit 1
    fi

    CURRENT_VERSION=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$REFERENCE_MANIFEST" | grep -o '[0-9]\+\.[0-9]\+\.[0-9]\+' | head -1)

    if [ -z "$CURRENT_VERSION" ]; then
        echo "ERROR: Could not parse version from $REFERENCE_MANIFEST" >&2
        exit 1
    fi

    IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"
    NEW_VERSION="${MAJOR}.${MINOR}.$((PATCH + 1))"

    for m in "${EXISTING_MANIFESTS[@]}"; do
        if [[ "$OSTYPE" == "darwin"* ]]; then
            sed -i '' "s/\"version\"[[:space:]]*:[[:space:]]*\"${CURRENT_VERSION}\"/\"version\": \"${NEW_VERSION}\"/" "$m"
        else
            sed -i "s/\"version\"[[:space:]]*:[[:space:]]*\"${CURRENT_VERSION}\"/\"version\": \"${NEW_VERSION}\"/" "$m"
        fi
        git add "$m"
        echo "Bumped $m: ${CURRENT_VERSION} -> ${NEW_VERSION}"
    done

    PLUGINS_BUMPED=1
    BUMPED_PLUGIN_NAMES+=("$PLUGIN_NAME")
    BUMPED_NEW_VERSION="$NEW_VERSION"
done

if [ "$PLUGINS_BUMPED" -eq 0 ]; then
    exit 0
fi

MARKETPLACE_JSON=".claude-plugin/marketplace.json"
if [ ! -f "$MARKETPLACE_JSON" ]; then
    echo "ERROR: $MARKETPLACE_JSON not found; cannot sync bumped plugin versions" >&2
    exit 1
fi
command -v node > /dev/null 2>&1 || { echo "ERROR: node not found; cannot sync $MARKETPLACE_JSON" >&2; exit 1; }

for i in "${!BUMPED_PLUGIN_NAMES[@]}"; do
    P="$MARKETPLACE_JSON" NAME="${BUMPED_PLUGIN_NAMES[$i]}" V="$BUMPED_NEW_VERSION" node -e '
      const fs = require("fs");
      const p = process.env.P;
      const data = JSON.parse(fs.readFileSync(p, "utf8"));
      const entry = (data.plugins || []).find((e) => e && e.name === process.env.NAME);
      if (!entry) {
        console.error(`ERROR: no plugins[] entry named ${process.env.NAME} in ${p}`);
        process.exit(1);
      }
      entry.version = process.env.V;
      fs.writeFileSync(p, JSON.stringify(data, null, 2) + "\n");
    '
    echo "Synced ${BUMPED_PLUGIN_NAMES[$i]} version in marketplace.json: $BUMPED_NEW_VERSION"
done

# If any plugins were bumped, also bump the marketplace.json metadata.version
M_MARKETPLACE_VERSION=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$MARKETPLACE_JSON" | head -1 | grep -o '[0-9]\+\.[0-9]\+\.[0-9]\+')
if [ -n "$M_MARKETPLACE_VERSION" ]; then
    IFS='.' read -r M_MAJOR M_MINOR M_PATCH <<< "$M_MARKETPLACE_VERSION"
    NEW_MARKETPLACE_VERSION="${M_MAJOR}.${M_MINOR}.$((M_PATCH + 1))"
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "0,/\"version\"[[:space:]]*:[[:space:]]*\"${M_MARKETPLACE_VERSION}\"/s//\"version\": \"${NEW_MARKETPLACE_VERSION}\"/" "$MARKETPLACE_JSON"
    else
        sed -i "0,/\"version\"[[:space:]]*:[[:space:]]*\"${M_MARKETPLACE_VERSION}\"/s//\"version\": \"${NEW_MARKETPLACE_VERSION}\"/" "$MARKETPLACE_JSON"
    fi
    echo "Bumped marketplace.json version: ${M_MARKETPLACE_VERSION} -> ${NEW_MARKETPLACE_VERSION}"
fi

git add "$MARKETPLACE_JSON"
exit 0
