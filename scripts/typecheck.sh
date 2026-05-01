#!/usr/bin/env bash
set -uo pipefail

# Run cargo check for the Rust CLI and TypeScript typecheck for the extension
# package in parallel. Uses tsgo when available, falling back to tsc.

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Prefer the rustup-managed cargo (supports edition 2024) over any system cargo
export PATH="$HOME/.cargo/bin:$PATH"

PIDS=()
EXIT=0

# --- Rust CLI typecheck ---
if [ -f "$WORKSPACE_ROOT/packages/cli/Cargo.toml" ]; then
  echo "Running cargo check for packages/cli..."
  (cd "$WORKSPACE_ROOT/packages/cli" && cargo check --quiet) &
  PIDS+=($!)
else
  echo "Warning: packages/cli/Cargo.toml not found, skipping cargo check." >&2
fi

# --- TypeScript extension typecheck ---
TSGO="$WORKSPACE_ROOT/node_modules/.bin/tsgo"
TSC="$WORKSPACE_ROOT/node_modules/.bin/tsc"

typecheck_ts_package() {
  local pkg_dir="$1"
  if [ ! -d "$pkg_dir" ]; then
    echo "Warning: $pkg_dir not found, skipping TypeScript typecheck." >&2
    return
  fi
  if [ -x "$TSGO" ]; then
    echo "Running tsgo --noEmit for $pkg_dir..."
    (cd "$pkg_dir" && "$TSGO" --noEmit) &
    PIDS+=($!)
  elif [ -x "$TSC" ]; then
    echo "Running tsc --noEmit for $pkg_dir..."
    (cd "$pkg_dir" && "$TSC" --noEmit) &
    PIDS+=($!)
  else
    echo "Error: Neither tsgo nor tsc found. Run yarn install to set up dependencies." >&2
    exit 1
  fi
}

typecheck_ts_package "$WORKSPACE_ROOT/packages/extension"
typecheck_ts_package "$WORKSPACE_ROOT/packages/claude-code-hooks"

for PID in "${PIDS[@]}"; do
  wait "$PID" || EXIT=1
done

exit $EXIT
