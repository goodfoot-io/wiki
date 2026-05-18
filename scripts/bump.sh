#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="$REPO_ROOT/packages/cli/package.json"
LEVEL="${1:-patch}"

case "$LEVEL" in
  major|minor|patch) ;;
  *)
    echo "Usage: yarn bump [major|minor|patch]" >&2
    exit 1
    ;;
esac

# Pass the path and level through the environment, never string-interpolated
# into the JS source: on Windows a path like `C:\Users\johnw` would have its
# backslashes consumed as JS escape sequences and silently corrupt the path.
NEW_VERSION=$(BUMP_SOURCE="$SOURCE" BUMP_LEVEL="$LEVEL" node -e '
  const fs = require("fs");
  const source = process.env.BUMP_SOURCE;
  const level = process.env.BUMP_LEVEL;
  const raw = fs.readFileSync(source, "utf8");
  const pkg = JSON.parse(raw);
  const [maj, min, pat] = pkg.version.split(".").map(Number);
  const next = level === "major" ? [maj + 1, 0, 0]
             : level === "minor" ? [maj, min + 1, 0]
             : [maj, min, pat + 1];
  const newVersion = next.join(".");
  const updated = raw.replace(/"version": "[^"]+"/, JSON.stringify("version") + ": " + JSON.stringify(newVersion));
  fs.writeFileSync(source, updated);
  console.log(newVersion);
')

echo "Bumped packages/cli to $NEW_VERSION"
echo ""
bash "$REPO_ROOT/scripts/sync-versions.sh"
