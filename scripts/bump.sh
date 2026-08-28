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

# Base the bump on the MAXIMUM product version across every manifest, not just
# packages/cli. The pre-commit.plugin-version.sh hook auto-bumps each changed
# plugin's plugins-{claude,codex,opencode,antigravity}/<name>/ manifests (and its
# marketplace.json plugins[] entry) on every commit that touches plugin files,
# so the plugin version routinely runs ahead of the CLI source-of-truth.
# Bumping only from
# the CLI version would then move the release version *backwards* relative to
# the plugin and republish an already-used number. Taking the max guarantees
# the new version is ahead of everything already shipped.
#
# Excluded on purpose: marketplace.json's metadata.version is the catalog's own
# version track (currently 1.0.x), independent of the wiki product version
# (0.5.x) that release.sh tags as wiki-v<version>. Folding it into the max would
# drag the product onto the catalog's track.
#
# Paths and the level pass through the environment, never string-interpolated
# into the JS source: on Windows a path like `C:\Users\johnw` would have its
# backslashes consumed as JS escape sequences and silently corrupt the path.
NEW_VERSION=$(REPO_ROOT="$REPO_ROOT" BUMP_LEVEL="$LEVEL" node -e '
  const fs = require("fs");
  const path = require("path");
  const root = process.env.REPO_ROOT;
  const level = process.env.BUMP_LEVEL;

  const versions = [];
  const addJsonVersion = (p) => {
    try {
      const v = JSON.parse(fs.readFileSync(p, "utf8")).version;
      if (typeof v === "string") versions.push(v);
    } catch { /* missing or unparseable manifest contributes no version */ }
  };

  // CLI (source of truth) + extension
  addJsonVersion(path.join(root, "packages/cli/package.json"));
  addJsonVersion(path.join(root, "packages/extension/package.json"));

  // npm platform packages
  const npmDir = path.join(root, "npm");
  if (fs.existsSync(npmDir)) {
    for (const d of fs.readdirSync(npmDir)) {
      addJsonVersion(path.join(npmDir, d, "package.json"));
    }
  }

  // Plugin manifests — the surface the auto-bump hook moves ahead of the CLI.
  // One plugin spans four platform trees with a per-platform manifest:
  //   plugins-claude/<name>/.claude-plugin/plugin.json
  //   plugins-codex/<name>/.codex-plugin/plugin.json
  //   plugins-opencode/<name>/package.json
  //   plugins-antigravity/<name>/plugin.json
  const platformTrees = [
    ["plugins-claude", ".claude-plugin/plugin.json"],
    ["plugins-codex", ".codex-plugin/plugin.json"],
    ["plugins-opencode", "package.json"],
    ["plugins-antigravity", "plugin.json"],
  ];
  for (const [dir, manifestRel] of platformTrees) {
    const treeDir = path.join(root, dir);
    if (!fs.existsSync(treeDir)) continue;
    for (const name of fs.readdirSync(treeDir)) {
      addJsonVersion(path.join(treeDir, name, manifestRel));
    }
  }

  // marketplace.json per-plugin entry versions (NOT metadata.version — see above).
  try {
    const market = JSON.parse(fs.readFileSync(path.join(root, ".claude-plugin/marketplace.json"), "utf8"));
    for (const p of (market.plugins || [])) {
      if (p && typeof p.version === "string") versions.push(p.version);
    }
  } catch { /* no marketplace manifest */ }

  const isSemver = (v) => /^\d+\.\d+\.\d+$/.test(v);
  const valid = versions.filter(isSemver);
  if (valid.length === 0) {
    console.error("bump: no valid semver versions found across manifests");
    process.exit(1);
  }

  const parse = (v) => v.split(".").map(Number);
  valid.sort((a, b) => {
    const [a1, a2, a3] = parse(a);
    const [b1, b2, b3] = parse(b);
    return a1 - b1 || a2 - b2 || a3 - b3;
  });
  const [maj, min, pat] = parse(valid[valid.length - 1]);
  const next = level === "major" ? [maj + 1, 0, 0]
             : level === "minor" ? [maj, min + 1, 0]
             : [maj, min, pat + 1];
  console.log(next.join("."));
')

# Write the computed version into the source of truth; sync-versions.sh fans it
# out to every other manifest.
P="$SOURCE" V="$NEW_VERSION" node -e '
  const fs = require("fs");
  const raw = fs.readFileSync(process.env.P, "utf8");
  const updated = raw.replace(/"version": "[^"]+"/, JSON.stringify("version") + ": " + JSON.stringify(process.env.V));
  fs.writeFileSync(process.env.P, updated);
'

echo "Bumped to $NEW_VERSION (max product version across manifests + $LEVEL)"
echo ""
bash "$REPO_ROOT/scripts/sync-versions.sh"
