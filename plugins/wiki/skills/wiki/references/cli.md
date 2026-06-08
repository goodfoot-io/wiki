# Wiki CLI Reference

**When to use this:** reaching past the day-to-day commands in `SKILL.md` — inspecting back-references, paginating search, validating specific files, machine-reading diagnostics, or wiring `wiki` into another tool.

The day-to-day commands (`wiki [query]`, `wiki check`) are documented in `SKILL.md`; this file covers everything else. `wiki check --fix` repairs drifted links, anchors, and frontmatter in place, and also creates meshes (anchors only, no why) for every uncovered fragment link. `--fix-dry-run` previews the plan without mutating anything; `--format json` emits structured diagnostics and is non-mutating.

---

## Inspection

```bash
wiki summary "Authorization"      # print a page's summary line
wiki list                         # all pages with title, aliases, tags, path
```

## Search pagination

```bash
wiki -l 10 "auth"                 # up to 10 results (default 3)
wiki -l 10 -o 10 "auth"           # next page
```

## Validation flags

```bash
cd wiki && wiki check             # scope validation to the wiki/ directory (selection follows CWD)
wiki check --no-exit-code         # report-only; exits 0 even with errors
wiki check --format json          # structured diagnostics
wiki check path/to/page.md        # validate specific globs only (resolved from CWD)
```

File selection follows the current working directory: bare `wiki check` validates every `*.md` page beneath the CWD, and explicit globs resolve from the CWD. Link, anchor, `git check-ignore`, and mesh resolution stay anchored at the git repository root, so a subdirectory check produces the same diagnostics as an equivalent repo-relative glob run from the repo root.

`--format json` is supported on most subcommands and is the right choice for any script consuming wiki output. The `wiki mesh` subcommands are the exception: they emit human-readable text only and reject `--format json` with a non-zero exit (`wiki mesh does not support --format json`).

## Document source

```bash
wiki --source worktree check      # default: working tree
wiki --source index    check      # staged content (use in pre-commit hooks)
wiki --source head     check      # latest commit (use in CI)
```

`--source` reads from a different snapshot of the repo without touching the working tree. The pre-commit hook in `git-hook-setup.md` uses `--source=worktree` for its `--fix` phase (you can only rewrite files read from the worktree).

## Fix flags — mesh coverage and link repair

```bash
wiki check --fix                  # repair drifted links/anchors/frontmatter AND create
                                  # meshes for uncovered fragment links; requires
                                  # --source=worktree (rewrites files on disk)
wiki check --fix --fix-dry-run    # preview created meshes + planned renames; no mutation
wiki check --fix --print-applied  # stdout = one repo-relative path per created/renamed
                                  # mesh; advisories → stderr
```

`--print-applied` is the pre-commit integration point: it lets the hook stage **exactly** the meshes this run created or renamed (`git add` each printed path) instead of a blanket `git add .mesh/`. Conflicts with `--fix-dry-run`. When a new slug path-collides with a pre-existing ancestor mesh, `--fix` renames the blocker to `<blocker>/<derived-leaf>` (or `<blocker>/index`), prints the blocker's new path for staging, and notes the rename on stderr.

## Mesh management (`wiki mesh`)

Inspect and reconcile `.wiki/` mesh coverage directly from the `wiki` CLI.

### `wiki mesh show <slug> [--patch]`

```bash
wiki mesh show billing/checkout    # list anchors: path, line range, stored hash, fresh/stale
wiki mesh show billing/checkout --patch  # also show a before/after diff for stale anchors
```

Prints each anchor's path, line range, stored rk64 hash, and whether it is fresh or stale (stale means the worktree content at that range no longer matches the stored hash). `--patch` adds a before/after diff for each stale anchor: the committed blob slice (from `HEAD`) versus the current worktree slice. When the committed copy cannot be reconstructed, the diff labels it explicitly rather than misrepresenting it: `(not in HEAD)` for a genuinely new file, or `(committed content unavailable: non-UTF-8 or unreadable)` when the path is in HEAD but its blob is not valid UTF-8. When an anchor is stale but `HEAD` already matches the worktree (the stored hash predates HEAD or reflects staged content), the diff prints a clarifying note instead of an empty patch.

### `wiki mesh add <slug> <anchor>... [--why <text>]`

```bash
# Create a new mesh (--why is required on first create).
# Coverage requires ONE mesh to anchor BOTH the wiki page and the code target,
# so create both anchors together:
wiki mesh add billing/checkout \
  wiki/checkout.md \
  packages/api/charge.ts#L30-L76 \
  --why "Checkout flow from wiki page to Stripe charge handler"

# Extend an existing mesh (--why is optional)
wiki mesh add billing/checkout packages/api/validate.ts#L1-L20

# Re-hash a stale anchor (upsert on exact path#range identity)
wiki mesh add billing/checkout packages/api/charge.ts#L30-L76

# Update the rationale only (no anchor): valid against an EXISTING mesh
wiki mesh add billing/checkout --why "Revised: now also covers refund path"
```

Anchors are specified as `path#Lstart-Lend` (line range) or a bare `path` (whole file, stored as the `0-0` sentinel). `add` upserts on exact `(path, start, end)` identity: it creates the mesh if it does not exist, appends a new anchor if the range is not present, or re-hashes an existing anchor if the range already exists. A batch `add` is atomic: every anchor is validated (bounds + file existence) before any write, so a single bad anchor leaves the store untouched.

**At least one anchor OR `--why` must be supplied.** The anchor-less form (`wiki mesh add <slug> --why "…"`) is a rationale-only update and requires the mesh to already exist (it errors otherwise — there is nothing to curate).

**`--why` is required when creating a new mesh** and is rejected as an error when the mesh does not yet exist and `--why` is absent. When the mesh already exists, `--why` is optional; if supplied it overwrites the stored rationale. Overwriting is never silent: when `--why` replaces a non-empty existing rationale, `add` prints `updated rationale for mesh \`<slug>\` (was: "…")` to stderr naming the previous value. Omitting `--why` preserves the stored rationale.

**Fail-closed behavior:** `add` exits non-zero if the target file is missing, the specified line range is out-of-bounds, or the path is not a valid repo-relative path.

### `wiki mesh remove <slug> [<anchor>]`

```bash
wiki mesh remove billing/checkout packages/api/old.ts#L5-L30   # remove one anchor
wiki mesh remove billing/checkout   # remove the whole mesh (all anchors)
```

With an anchor argument, removes that single anchor from the mesh. If removing the anchor leaves the mesh empty, the mesh file is deleted. Without an anchor argument, removes the entire mesh file. `remove` is idempotent: a missing anchor or a missing mesh prints a `nothing to remove` notice to stderr and exits 0. This is what makes the reconciliation sequence `wiki mesh add <new>` then `wiki mesh remove <stale>` safe to re-run.

---

## Global flags

| Flag | Effect |
|---|---|
| `-v`, `--version` | Print the CLI version. |
| `--perf` | Emit per-event timings to stderr (also: `WIKI_PERF=1`). |
| `--format json` | Structured output (subcommand-dependent; rejected by `wiki mesh`). |
| `--source <s>` | `worktree` (default) / `index` / `head`. |
| `-l <N>` / `-o <N>` | Search result limit / offset. |

## Reserved titles

`title` and `aliases` may not be any of: `check`, `pin`, `stale`, `links`, `list`, `summary`, `print`. The bare `wiki <title>` form would otherwise dispatch to the subcommand instead of the page.
