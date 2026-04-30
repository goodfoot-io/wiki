#!/usr/bin/env bash
# PostToolUse hook: run `wiki check <file>` on edited markdown files
# that are part of a wiki (ancestor has wiki.toml) or are *.wiki.md.
set -uo pipefail

input=$(cat)

file_path=$(printf '%s' "$input" | jq -r '
  .tool_input.file_path
  // .tool_input.notebook_path
  // empty
')

if [[ -z "$file_path" ]]; then
  exit 0
fi

if [[ ! -e "$file_path" ]]; then
  exit 0
fi

# Only consider markdown files.
case "$file_path" in
  *.md) ;;
  *) exit 0 ;;
esac

is_wiki=0

# Case 2: *.wiki.md
case "$file_path" in
  *.wiki.md) is_wiki=1 ;;
esac

# Case 1: any *.md whose ancestor directory contains wiki.toml
if [[ $is_wiki -eq 0 ]]; then
  dir=$(dirname -- "$file_path")
  while [[ "$dir" != "/" && -n "$dir" ]]; do
    if [[ -f "$dir/wiki.toml" ]]; then
      is_wiki=1
      break
    fi
    parent=$(dirname -- "$dir")
    if [[ "$parent" == "$dir" ]]; then
      break
    fi
    dir="$parent"
  done
fi

if [[ $is_wiki -eq 0 ]]; then
  exit 0
fi

if ! command -v wiki >/dev/null 2>&1; then
  exit 0
fi

output=$(wiki check "$file_path" 2>&1)
status=$?

if [[ $status -ne 0 ]]; then
  message=$(printf 'wiki check failed for %s:\n%s' "$file_path" "$output")
  jq -n \
    --arg msg "$message" \
    '{
      decision: "block",
      reason: $msg,
      systemMessage: $msg,
      hookSpecificOutput: {
        hookEventName: "PostToolUse",
        additionalContext: $msg
      }
    }'
  exit 0
fi

exit 0
