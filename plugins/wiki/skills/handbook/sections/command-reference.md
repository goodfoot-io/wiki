# Command reference

Flat lookup. For the day-to-day flow see the other sections; this is the surface.

## Subcommands

| Command | Purpose |
|---|---|
| `wiki [query]` | Default. Ranked search over titles/summaries. |
| `wiki list [--tag T] [--limit N] [--offset N]` | All pages with title, aliases, tags, path. |
| `wiki summary <title\|alias\|path>` | Print a page's summary (`{title,file,summary}` under `--format json`); reads stdin if omitted. |
| `wiki check [globs…]` | Validate links, frontmatter, mesh coverage. |
| `wiki mesh show <slug> [--patch]` | Anchors with path, range, hash, fresh/stale; `--patch` diffs committed vs worktree for stale anchors. |
| `wiki mesh add <slug> [<anchor>…] [--why <text>]` | Create / extend / upsert a mesh. |
| `wiki mesh remove <slug> [<anchor>]` | Remove one anchor, or the whole mesh if no anchor given. |

## `check` flags

| Flag | Effect |
|---|---|
| `--fix` | Rewrite drifted links/anchors/frontmatter AND create coverage for uncovered links. Requires `--source=worktree`. |
| `--fix-dry-run` | Preview created meshes + planned renames; no mutation. Requires `--fix`. |
| `--print-applied` | stdout = one repo-relative path per created/renamed mesh; advisories → stderr. Requires `--fix`; conflicts with `--fix-dry-run`. |
| `--no-exit-code` | Exit 0 even with errors (report-only). |

## Global flags

| Flag | Effect |
|---|---|
| `-l N` / `-o N` | Search limit (default 3) / offset. |
| `--format json` | Structured output. **Rejected by `wiki mesh`** (human-readable only). |
| `--source worktree\|index\|head` | Snapshot to read. Default `worktree`. `--fix` needs `worktree`. |
| `-v`, `--version` | Print CLI version. |
| `--perf` (or `WIKI_PERF=1`) | Per-event timings to stderr. |

## Anchor grammar

`path#Lstart-Lend` (line range) or bare `path` (whole file, stored as `0-0` sentinel). Paths are repo-relative-resolvable per markdown rules.

## `wiki mesh add` semantics

- Upserts on exact `(path, start, end)`: creates the mesh, appends a new anchor, or re-hashes an existing one. Always recomputes the hash.
- Batch `add` is **atomic**: all anchors validated (bounds + existence) before any write; one bad anchor leaves the store untouched.
- **`--why` is required when creating** a new mesh; optional after. Overwriting a non-empty rationale prints `updated rationale for mesh <slug> (was: "…")` to stderr — never silent.
- Anchor-less form `wiki mesh add <slug> --why "…"` is a rationale-only update; errors if the mesh doesn't exist.
- At least one anchor **or** `--why` must be supplied.
- **Fail-closed:** non-zero exit on missing target file, out-of-bounds range, or invalid path.

## `wiki mesh remove` semantics

- With an anchor: removes it; deletes the mesh file if that empties it.
- Without: removes the whole mesh.
- **Idempotent:** missing anchor/mesh prints `nothing to remove` and exits 0 — which is what makes `add <new>` then `remove <old>` safe to re-run.

## Reserved titles

`title` and aliases may not be: `check`, `pin`, `stale`, `links`, `list`, `summary`, `print`.
