---
title: Wiki Mesh Integration
summary: Design for wiki check and wiki check --fix — commands that bridge wiki fragment links with git mesh drift detection.
tags:
  - tooling
  - git-mesh
---

Wiki fragment links (`[label](path#L10-L20)`) are navigation — they point at code but carry no staleness signal of their own. The mesh integration closes that gap by requiring every fragment link to have a corresponding [git mesh](https://github.com/git-mesh/git-mesh) anchor. `git mesh` then handles drift detection independently: when anchored content changes, `git mesh stale` reports it.

Two commands implement this:

- **`wiki check`** — validates that each fragment link has a covering mesh anchor; fails if any are missing.
- **`wiki check --fix`** — in addition to repairing drifted links, anchors, and frontmatter, creates git meshes for all fragment links not yet covered by a mesh.

## wiki check

```bash
wiki check
wiki check wiki/architecture/*.md
wiki check "packages/auth/**/*.md"
```

Extends the existing `wiki check` validation pass with a [mesh coverage check](/packages/cli/src/commands/mesh_coverage.rs#L55-L58). For each internal fragment link with a line range, it [runs `git mesh list`](/packages/cli/src/commands/mesh_coverage.rs#L147-L147) `<path>#L<s>-L<e> --porcelain` and verifies that at least one returned mesh also anchors the wiki file containing the link. Any uncovered link is reported as an error ([non-zero exit](/packages/cli/src/commands/mesh_coverage.rs#L106-L116)).

Mesh coverage is always on; `git mesh` must be installed or `wiki check` fails fast. Glob targeting follows the same rules as bare `wiki check`: a markdown file is treated as a wiki page only when its frontmatter has both a non-empty `title` and `summary`; omitting globs walks all `.md` files under `$WIKI_DIR` (defaulting to `wiki`) applying that filter.

## wiki check --fix (mesh coverage)

```bash
wiki check --fix
wiki check --fix wiki/architecture/*.md
wiki check --fix "packages/auth/**/*.md"
```

[Scans the same file set as `wiki check`](/packages/cli/src/commands/mesh/scaffold.rs#L165-L172) and creates a `git mesh` for every fragment link not yet covered. Runs as Fix #4 within the `--fix` pipeline, after link and anchor repairs. The meshes are created with anchors only (no why); the author is expected to add a `git mesh why` before or after committing (see [Adding Mesh Coverage](../guides/adding-mesh-coverage.md)).

### Mesh naming

Names follow the `wiki/<page-title-slug>/<target-slug>` convention:

- **Page title slug** — derived from the wiki page's frontmatter `title` field (falling back to the filename stem). This keeps names stable across file renames.
- **Target slug** — derived from the link label ([truncated at five words](/packages/cli/src/commands/mesh/draft.rs#L166-L166), falling back to the target file stem for long or path-style labels).

Names are topical, not path-derived: one wiki page will typically produce several meshes covering different subsystems. Authors are expected to rename generated slugs to match the conceptual relationship before committing.

### Why generation

When `--fix` creates a mesh, the `why` is extracted from the prose sentence containing the link, with all markdown syntax stripped. This produces a first-draft definition of the subsystem the anchors collectively form. Per the git mesh handbook:

> Write the **why** as a definition: name the subsystem the anchors collectively form and say plainly what it does across them.

Generated whys require author review — sentences that started with a backtick identifier produce headless predicates, and bullet-list summary lines produce terse fragments. The fix pass inserts the link label as a reconstructed subject when it detects a headless verb.

### Default glob behavior

Omitting globs walks all `.md` files and treats those whose frontmatter has both a non-empty `title` and `summary` as wiki pages. `$WIKI_DIR` defaults to `wiki`. This matches the default discovery behavior used by all other wiki commands (see [discover_files](/packages/cli/src/commands/mod.rs#L143-L185)).

### Missing-path filtering

Before creating any mesh, `--fix` verifies that every anchor's path exists at the chosen source. If a wiki link's target is missing, the mesh is dropped — a partial mesh with the bad anchor stripped is never created, because losing an anchor changes what the mesh means.

Path existence is resolved against the active `--source`:

- `--source=worktree` (default) checks `repo_root/<path>` on the filesystem.
- `--source=index` and `--source=head` check membership in that source's tracked-path list, so a worktree-only deletion does not invalidate a mesh whose target still lives in the index or in HEAD.

Dropped meshes are surfaced as an advisory: `Skipped mesh \`<slug>\` — references missing path \`<path>\`.`

Fix the wiki link (correct the path, or remove the link if the target is intentionally gone) and rerun `wiki check --fix`.

### --print-applied

`--print-applied` routes one repo-relative path per created or renamed mesh to stdout, while all advisories go to stderr. The pre-commit hook uses this to stage **exactly** the meshes this run created or renamed without a blanket `git add .mesh/`.

Use `wiki check --fix --fix-dry-run` to preview what would be created and any planned renames without mutating `.mesh/`.

## Workflow

```bash
# 1. Check for uncovered links
wiki check

# 2. Create mesh coverage in one pass
wiki check --fix

# 3. Review auto-created meshes; add git mesh why for each
git mesh why wiki/<slug> -m "Definition of what this mesh covers."

# 4. Validate coverage
wiki check
```

## References

- [discover_files](/packages/cli/src/commands/mod.rs#L143-L185)
