#!/bin/bash
# Gate: the four version-bearing surfaces must carry one identical version:
#   plugins-claude/<name>/.claude-plugin/plugin.json
#   plugins-codex/<name>/.codex-plugin/plugin.json
#   plugins-opencode/<name>/package.json
#   plugins-antigravity/<name>/plugin.json
#   .claude-plugin/marketplace.json (plugins[] entry for <name>)
# A MISSING expected surface is a hard failure, not a skip. .agents/plugins/
# marketplace.json is checked for the correct source path only — its entries
# carry no version fields. Fail closed: any problem aborts the commit.
set -euo pipefail

MARKETPLACE_JSON=".claude-plugin/marketplace.json"
AGENTS_MARKETPLACE_JSON=".agents/plugins/marketplace.json"

command -v node > /dev/null 2>&1 || {
    echo "ERROR: node not found; cannot verify plugin version consistency" >&2
    exit 1
}

[ -f "$MARKETPLACE_JSON" ] || { echo "ERROR: $MARKETPLACE_JSON not found" >&2; exit 1; }
[ -f "$AGENTS_MARKETPLACE_JSON" ] || { echo "ERROR: $AGENTS_MARKETPLACE_JSON not found" >&2; exit 1; }

if MARKETPLACE_JSON="$MARKETPLACE_JSON" AGENTS_MARKETPLACE_JSON="$AGENTS_MARKETPLACE_JSON" node -e '
  const fs = require("fs");
  const errors = [];

  const readJson = (p) => {
    try {
      return JSON.parse(fs.readFileSync(p, "utf8"));
    } catch (err) {
      errors.push(`${p}: unparseable JSON (${err.message})`);
      return null;
    }
  };

  const market = readJson(process.env.MARKETPLACE_JSON);
  const agentsMarket = readJson(process.env.AGENTS_MARKETPLACE_JSON);
  if (!market || !agentsMarket) {
    report();
  }

  function report() {
    for (const e of errors) console.error(`ERROR: ${e}`);
    if (errors.length > 0) process.exit(1);
  }

  const pluginEntries = (market.plugins || []).filter(
    (p) => p && typeof p.name === "string" && typeof p.version === "string"
  );
  if (pluginEntries.length === 0) {
    errors.push(`${process.env.MARKETPLACE_JSON}: no plugins[] entries with name and version`);
    report();
  }

  for (const entry of pluginEntries) {
    const surfaces = [
      [`plugins-claude/${entry.name}/.claude-plugin/plugin.json`, "json"],
      [`plugins-codex/${entry.name}/.codex-plugin/plugin.json`, "json"],
      [`plugins-opencode/${entry.name}/package.json`, "json"],
      [`plugins-antigravity/${entry.name}/plugin.json`, "json"],
    ];
    for (const [p] of surfaces) {
      if (!fs.existsSync(p)) {
        errors.push(`${p}: expected version-bearing manifest is missing`);
        continue;
      }
      const doc = readJson(p);
      if (doc === null) continue;
      if (typeof doc.version !== "string") {
        errors.push(`${p}: no version field`);
        continue;
      }
      if (doc.version !== entry.version) {
        errors.push(`version mismatch for ${entry.name}: ${p} has ${doc.version}, marketplace has ${entry.version}`);
      }
    }
  }

  // Codex discovery wiring: source path must point at the platform tree.
  // Entries carry no version fields and gain none — never checked here.
  const agentEntries = agentsMarket.plugins || [];
  if (agentEntries.length === 0) {
    errors.push(`${process.env.AGENTS_MARKETPLACE_JSON}: no plugins[] entries`);
  }
  for (const entry of agentEntries) {
    if (!entry || typeof entry.name !== "string") {
      errors.push(`${process.env.AGENTS_MARKETPLACE_JSON}: plugins[] entry without a name`);
      continue;
    }
    const source = entry.source;
    const path = typeof source === "string" ? source : source && source.path;
    if (typeof path !== "string") {
      errors.push(`${process.env.AGENTS_MARKETPLACE_JSON}: plugin ${entry.name} has no source path`);
      continue;
    }
    const expected = `./plugins-codex/${entry.name}`;
    if (path !== expected) {
      errors.push(`${process.env.AGENTS_MARKETPLACE_JSON}: plugin ${entry.name} source path is ${path}, expected ${expected}`);
    }
  }

  report();
' ; then
    exit 0
fi

echo ""
echo "Commit blocked: plugin versions must match across all four version-bearing surfaces."
exit 1
