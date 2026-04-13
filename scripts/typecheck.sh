#!/usr/bin/env bash
set -uo pipefail

# Run cargo check for the Rust CLI and TypeScript typecheck for the extension
# package in parallel. Uses tsgo when available, falling back to tsc.

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

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

if [ -d "$WORKSPACE_ROOT/packages/extension" ]; then
  if [ -x "$TSGO" ]; then
    echo "Running tsgo --noEmit for packages/extension..."
    (cd "$WORKSPACE_ROOT/packages/extension" && "$TSGO" --noEmit) &
    PIDS+=($!)
  elif [ -x "$TSC" ]; then
    echo "Running tsc --noEmit for packages/extension..."
    (cd "$WORKSPACE_ROOT/packages/extension" && "$TSC" --noEmit) &
    PIDS+=($!)
  else
    echo "Error: Neither tsgo nor tsc found. Run yarn install to set up dependencies." >&2
    exit 1
  fi
else
  echo "Warning: packages/extension not found, skipping TypeScript typecheck." >&2
fi

for PID in "${PIDS[@]}"; do
  wait "$PID" || EXIT=1
done

exit $EXIT
