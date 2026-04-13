#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI_PKG="$REPO_ROOT/packages/cli/package.json"

# --- Read version ---
VERSION=$(node -pe "require('$CLI_PKG').version")

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "ERROR: Version '$VERSION' in packages/cli/package.json is not valid semver." >&2
  exit 1
fi

TAG="wiki-v$VERSION"

# --- Branch check ---
BRANCH=$(git -C "$REPO_ROOT" rev-parse --abbrev-ref HEAD)
if [[ "$BRANCH" != "main" ]]; then
  echo "ERROR: Releases must be run from the 'main' branch (currently on '$BRANCH')." >&2
  exit 1
fi

# --- Tag existence check ---
if git -C "$REPO_ROOT" rev-parse "$TAG" >/dev/null 2>&1; then
  echo "ERROR: Tag '$TAG' already exists locally." >&2
  exit 1
fi
if git -C "$REPO_ROOT" ls-remote --tags origin "$TAG" | grep -q "$TAG"; then
  echo "ERROR: Tag '$TAG' already exists on remote." >&2
  exit 1
fi

# --- Uncommitted changes check ---
if ! git -C "$REPO_ROOT" diff --quiet || ! git -C "$REPO_ROOT" diff --cached --quiet; then
  echo "ERROR: There are uncommitted changes. Commit or stash them before releasing." >&2
  exit 1
fi

echo "Releasing $TAG from branch '$BRANCH'..."

git -C "$REPO_ROOT" tag "$TAG"
git -C "$REPO_ROOT" push origin "$TAG"

echo "Done. Tag $TAG pushed."
