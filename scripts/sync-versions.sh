#!/usr/bin/env bash
set -euo pipefail

# Every Node invocation here receives paths and values through the environment,
# never string-interpolated into the JS source. On Windows a path such as
# `C:\Users\johnw` interpolated into a JS string literal has its backslashes
# consumed as escape sequences (`\U`, `\j`, ...) and silently corrupts, which
# previously broke `yarn bump`.

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="$REPO_ROOT/packages/cli/package.json"

if [ ! -f "$SOURCE" ]; then
  echo "Error: Source package.json not found at $SOURCE" >&2
  exit 1
fi

read_version() {
  P="$1" node -e 'console.log(JSON.parse(require("fs").readFileSync(process.env.P,"utf8")).version || "")'
}

set_version() {
  P="$1" V="$2" node -e '
    const fs = require("fs");
    const pkg = JSON.parse(fs.readFileSync(process.env.P, "utf8"));
    pkg.version = process.env.V;
    fs.writeFileSync(process.env.P, JSON.stringify(pkg, null, 2) + "\n");
  '
}

VERSION=$(read_version "$SOURCE")

if [ -z "$VERSION" ]; then
  echo "Error: Could not read version from $SOURCE" >&2
  exit 1
fi

echo "Source of truth: $SOURCE"
echo "Version: $VERSION"
echo ""

updated=0

# Update npm platform packages
for pkg_dir in "$REPO_ROOT"/npm/wiki-*/; do
  pkg_json="$pkg_dir/package.json"
  if [ -f "$pkg_json" ]; then
    current=$(read_version "$pkg_json")
    if [ "$current" != "$VERSION" ]; then
      set_version "$pkg_json" "$VERSION"
      echo "Updated: $pkg_json ($current -> $VERSION)"
      updated=$((updated + 1))
    else
      echo "OK:      $pkg_json (already $VERSION)"
    fi
  fi
done

# Update optionalDependencies in packages/cli/package.json
cli_json="$REPO_ROOT/packages/cli/package.json"
if [ -f "$cli_json" ]; then
  result=$(P="$cli_json" V="$VERSION" node -e '
    const fs = require("fs");
    const pkg = JSON.parse(fs.readFileSync(process.env.P, "utf8"));
    let changed = false;
    if (pkg.optionalDependencies) {
      for (const [name, ver] of Object.entries(pkg.optionalDependencies)) {
        if (ver !== process.env.V) {
          pkg.optionalDependencies[name] = process.env.V;
          changed = true;
        }
      }
    }
    if (changed) {
      fs.writeFileSync(process.env.P, JSON.stringify(pkg, null, 2) + "\n");
    }
    process.stdout.write(changed ? "updated" : "ok");
  ')
  echo ""
  if [ "$result" = "updated" ]; then
    echo "Updated: $cli_json optionalDependencies -> $VERSION"
    updated=$((updated + 1))
  else
    echo "OK:      $cli_json optionalDependencies (already $VERSION)"
  fi
fi

# Update packages/extension/package.json
ext_json="$REPO_ROOT/packages/extension/package.json"
if [ -f "$ext_json" ]; then
  current=$(read_version "$ext_json")
  if [ "$current" != "$VERSION" ]; then
    set_version "$ext_json" "$VERSION"
    echo "Updated: $ext_json ($current -> $VERSION)"
    updated=$((updated + 1))
  else
    echo "OK:      $ext_json (already $VERSION)"
  fi
fi

# Update packages/cli/Cargo.toml so the compiled binary's --version matches.
cargo_toml="$REPO_ROOT/packages/cli/Cargo.toml"
if [ -f "$cargo_toml" ]; then
  current=$(awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/"/, "", $3); print $3; exit}' "$cargo_toml")
  if [ -n "$current" ] && [ "$current" != "$VERSION" ]; then
    # Replace only the [package] version line, not dependency versions.
    awk -v ver="$VERSION" '
      BEGIN { in_pkg = 0; replaced = 0 }
      /^\[package\]/ { in_pkg = 1; print; next }
      /^\[/ && !/^\[package\]/ { in_pkg = 0; print; next }
      in_pkg && !replaced && /^version[[:space:]]*=/ {
        print "version = \"" ver "\""
        replaced = 1
        next
      }
      { print }
    ' "$cargo_toml" > "$cargo_toml.tmp" && mv "$cargo_toml.tmp" "$cargo_toml"
    echo "Updated: $cargo_toml ($current -> $VERSION)"
    updated=$((updated + 1))
  else
    echo "OK:      $cargo_toml (already $VERSION)"
  fi
fi

# Refresh Cargo.lock so the wiki entry matches the new [package] version.
# CI uses `cargo build --locked` which fails if Cargo.lock is out of sync.
cargo_lock="$REPO_ROOT/packages/cli/Cargo.lock"
if [ -f "$cargo_lock" ] && [ -f "$cargo_toml" ]; then
  lock_version=$(awk '
    /^\[\[package\]\]/ { in_pkg = 1; name = ""; next }
    in_pkg && /^name[[:space:]]*=[[:space:]]*"wiki"$/ { name = "wiki"; next }
    in_pkg && name == "wiki" && /^version[[:space:]]*=/ {
      gsub(/"/, "", $3); print $3; exit
    }
    /^$/ { in_pkg = 0; name = "" }
  ' "$cargo_lock")
  if [ "$lock_version" != "$VERSION" ]; then
    (
      cd "$REPO_ROOT/packages/cli" && \
      env PATH="$HOME/.cargo/bin:$PATH" \
        CARGO_TARGET_DIR="${WIKI_CARGO_TARGET_ROOT:-$HOME/.cache/wiki/cargo-target}/sync" \
        cargo update --workspace --quiet
    )
    echo "Updated: $cargo_lock ($lock_version -> $VERSION)"
    updated=$((updated + 1))
  else
    echo "OK:      $cargo_lock (already $VERSION)"
  fi
fi

# Update plugin manifests across the three platform plugin trees:
#   plugins-claude/<name>/.claude-plugin/plugin.json
#   plugins-codex/<name>/.codex-plugin/plugin.json
#   plugins-opencode/<name>/package.json
for platform_manifest in \
  "claude .claude-plugin/plugin.json" \
  "codex .codex-plugin/plugin.json" \
  "opencode package.json"
do
  platform="${platform_manifest%% *}"
  manifest_rel="${platform_manifest#* }"
  for plugin_dir in "$REPO_ROOT"/plugins-$platform/*/; do
    [ -d "$plugin_dir" ] || continue
    plugin_json="${plugin_dir}${manifest_rel}"
    if [ -f "$plugin_json" ]; then
      current=$(read_version "$plugin_json")
      if [ -n "$current" ] && [ "$current" != "$VERSION" ]; then
        set_version "$plugin_json" "$VERSION"
        echo "Updated: $plugin_json ($current -> $VERSION)"
        updated=$((updated + 1))
      else
        echo "OK:      $plugin_json (already $VERSION)"
      fi
    fi
  done
done

# Update marketplace manifest at .claude-plugin/marketplace.json
market_json="$REPO_ROOT/.claude-plugin/marketplace.json"
if [ -f "$market_json" ]; then
  result=$(P="$market_json" V="$VERSION" node -e '
    const fs = require("fs");
    const data = JSON.parse(fs.readFileSync(process.env.P, "utf8"));
    let changed = false;
    for (const p of (data.plugins || [])) {
      if (p && Object.prototype.hasOwnProperty.call(p, "version") && p.version !== process.env.V) {
        p.version = process.env.V;
        changed = true;
      }
    }
    if (changed) {
      fs.writeFileSync(process.env.P, JSON.stringify(data, null, 2) + "\n");
    }
    process.stdout.write(changed ? "updated" : "ok");
  ')
  if [ "$result" = "updated" ]; then
    echo "Updated: $market_json -> $VERSION"
    updated=$((updated + 1))
  else
    echo "OK:      $market_json (already $VERSION)"
  fi
fi

echo ""
echo "Done. $updated file(s) updated to version $VERSION."
