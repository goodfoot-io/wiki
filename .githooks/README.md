# Git hooks

This repo uses a **router / dispatcher** model for git hooks, wired via
`git config core.hooksPath .githooks` (tracked, reviewable — never the
untracked `.git/hooks/`).

## Architecture

Each git event has **one thin dispatcher** (`pre-commit`, `post-commit`) that
contains no business logic — only an ordered `PARTS` list and a run loop.
Each concern is **one sub-script** named `<event>.<concern>.sh`. Adding,
removing, or reordering a behavior is editing the `PARTS` list and dropping
in one file.

Events are classified and the classes never mix:

- **Fail-closed** (`pre-commit`): a non-zero sub-script aborts the commit.
- **Advisory** (`post-commit`): a sub-script failure is reported but never
  aborts later parts or the commit that already landed.

Sub-scripts degrade gracefully (no-op silently if their tool is absent),
are independently runnable, and `bash -n`-clean. Auto-fixing sub-scripts
`git add` their fixes **before** any gate.

## Sub-scripts

| Sub-script | Event | Purpose | Blocking? |
|---|---|---|---|
| `pre-commit.wiki.sh` | pre-commit | `wiki check --fix --print-applied --no-exit-code`: auto-fixes drifted links/frontmatter and creates mesh coverage in one pass; re-stages fixed `.md` files and created/renamed meshes | No |
| `pre-commit.biome.sh` | pre-commit | `biome check --fix` on staged TS/JS; re-stage fixes | No |
| `pre-commit.plugin-version.sh` | pre-commit | Bump changed plugins' versions + sync marketplace.json; re-stage | No |
| `pre-commit.version-consistency.sh` | pre-commit | Gate: marketplace.json versions must match plugin.json | **Yes** |

`merge-json-version` is a custom git **merge driver** (configured via
gitattributes/config), not a hook event — it is not part of the dispatcher
model and is left standalone.

## Adding a concern

1. Write `.githooks/<event>.<concern>.sh` (start from an existing sub-script:
   `command -v <tool> || exit 0` guard; auto-fixers re-stage before gating;
   wrap status-bearing calls in `set +e`/`set -e`).
2. `chmod +x` it and add its filename to the dispatcher's `PARTS` array in
   the position it should run.
3. Verify: `bash -n` clean, `git ls-files --stage` shows `100755`.
