#!/usr/bin/env bash
set -euo pipefail

bin="$(pwd)/target/release/wiki"
ext_root="$HOME/.vscode-server/data/User/globalStorage/goodfoot.wiki-extension/bin"

if [ -d "$ext_root" ]; then
  for d in "$ext_root"/*/linux-arm64; do
    [ -d "$(dirname "$d")" ] || continue
    mkdir -p "$d"
    ln -sf "$bin" "$d/wiki"
  done
fi

if w=$(command -v wiki 2>/dev/null); then
  if [ -L "$w" ] || [ -w "$w" ] || [ -w "$(dirname "$w")" ]; then
    ln -sf "$bin" "$w"
  fi
fi
