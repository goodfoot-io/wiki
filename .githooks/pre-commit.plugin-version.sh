#!/bin/bash
# Auto-bump patch version of any changed plugin's manifests, keeping every
# platform manifest of that plugin at one shared version, sync that version
# into marketplace.json, and bump marketplace.json's own version.
# Staging is hunk-scoped at the index level: only the version fields are
# swapped into the index, so unrelated manifest edits — staged or merely
# unstaged in the worktree — never ride along in the commit. Non-blocking
# (consistency is gated separately in pre-commit.version-consistency.sh).
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

# Stage ONLY the version hunk of $1: rebuild the staged blob from the file's
# INDEX copy (worktree copy as fallback for not-yet-tracked files), apply the
# old->new version replacement, and swap it into the index via a temp blob.
# The worktree is never touched here, so unrelated unstaged edits survive
# untouched and unrelated staged edits are preserved verbatim in the blob base.
stage_version_hunk() {
    local path="$1" old="$2" new="$3"
    local tmp blob
    tmp="$(mktemp "${TMPDIR:-/tmp}/plugin-version-hunk.XXXXXX")"
    { git show ":$path" 2>/dev/null || cat "$path"; } 2>/dev/null \
        | sed "s/\"version\"[[:space:]]*:[[:space:]]*\"${old}\"/\"version\": \"${new}\"/" \
        > "$tmp"
    if ! grep -q "\"version\": \"${new}\"" "$tmp"; then
        # Replacement did not land on the index base (untracked or unusually
        # formatted manifest) — fall back to the already-bumped worktree copy,
        # matching the pre-index-level behavior for that case.
        cat "$path" > "$tmp"
    fi
    blob=$(git hash-object -w "$tmp")
    rm -f "$tmp"
    git update-index --cacheinfo "100644,$blob,$path"
}

# First-occurrence "version" field bump shared by the worktree side and the
# index-replay side of the marketplace update. One mechanism for this class
# of replacement, and it lives in node rather than sed: the first-match
# address form a streaming sed would need here is GNU-only — BSD sed rejects
# it as a compile error, which under set -e aborted macOS commits mid-staging
# after the manifests were already index-swapped. node is already a hard
# requirement of this hook. Exits non-zero when the file's first version is
# not $old, unless ALLOW_MISSING=1 (used by the tolerant index-base replay).
bump_first_marketplace_version() {
    local file="$1" old="$2" new="$3"
    P="$file" OLD="$old" NEW="$new" ALLOW_MISSING="${ALLOW_MISSING:-0}" node -e '
      const fs = require("fs");
      const p = process.env.P;
      const raw = fs.readFileSync(p, "utf8");
      const match = /"version"\s*:\s*"([^"]*)"/.exec(raw);
      if (!match || match[1] !== process.env.OLD) {
        if (process.env.ALLOW_MISSING === "1") process.exit(0);
        console.error(`ERROR: ${p}: first version is ${match ? match[1] : "absent"}, expected ${process.env.OLD}`);
        process.exit(1);
      }
      fs.writeFileSync(
        p,
        raw.slice(0, match.index) + `"version": "${process.env.NEW}"` + raw.slice(match.index + match[0].length)
      );
    '
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
        stage_version_hunk "$m" "$CURRENT_VERSION" "$NEW_VERSION"
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
NEW_MARKETPLACE_VERSION=""
if [ -n "$M_MARKETPLACE_VERSION" ]; then
    IFS='.' read -r M_MAJOR M_MINOR M_PATCH <<< "$M_MARKETPLACE_VERSION"
    NEW_MARKETPLACE_VERSION="${M_MAJOR}.${M_MINOR}.$((M_PATCH + 1))"
    bump_first_marketplace_version "$MARKETPLACE_JSON" "$M_MARKETPLACE_VERSION" "$NEW_MARKETPLACE_VERSION"
    echo "Bumped marketplace.json version: ${M_MARKETPLACE_VERSION} -> ${NEW_MARKETPLACE_VERSION}"
fi

# Stage ONLY the marketplace hunks: rebuild from the INDEX copy (worktree copy
# as fallback for a not-yet-tracked marketplace), replay the entry syncs and
# the metadata.version bump onto that base, and swap the result into the
# index — unrelated marketplace edits, staged or unstaged, never ride along.
STAGED_MARKET="$(mktemp "${TMPDIR:-/tmp}/plugin-version-market.XXXXXX")"
{ git show ":$MARKETPLACE_JSON" 2>/dev/null || cat "$MARKETPLACE_JSON"; } 2>/dev/null > "$STAGED_MARKET"
for i in "${!BUMPED_PLUGIN_NAMES[@]}"; do
    P="$STAGED_MARKET" NAME="${BUMPED_PLUGIN_NAMES[$i]}" V="$BUMPED_NEW_VERSION" node -e '
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
done
if [ -n "$M_MARKETPLACE_VERSION" ] && [ -n "$NEW_MARKETPLACE_VERSION" ]; then
    # Tolerant on the index base: an unrelated staged metadata.version edit
    # makes its first version differ from ours — keep our hunk out rather
    # than aborting the commit mid-staging.
    ALLOW_MISSING=1 bump_first_marketplace_version "$STAGED_MARKET" "$M_MARKETPLACE_VERSION" "$NEW_MARKETPLACE_VERSION"
fi
MARKET_BLOB=$(git hash-object -w "$STAGED_MARKET")
rm -f "$STAGED_MARKET"
git update-index --cacheinfo "100644,$MARKET_BLOB,$MARKETPLACE_JSON"
exit 0
