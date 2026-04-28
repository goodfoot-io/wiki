---
title: Wiki Mesh Integration
summary: Design for wiki check --mesh and wiki mesh scaffold — commands that bridge wiki fragment links with git mesh drift detection.
tags:
  - tooling
  - git-mesh
---

Wiki fragment links (`[label](path#L10-L20)`) are navigation — they point at code but carry no staleness signal of their own. The mesh integration closes that gap by requiring every fragment link to have a corresponding [git mesh](https://github.com/git-mesh/git-mesh) anchor. `git mesh` then handles drift detection independently: when anchored content changes, `git mesh stale` reports it.

Two commands implement this:

- **`wiki check --mesh`** — validates that each fragment link has a covering mesh anchor; fails if any are missing.
- **`wiki mesh scaffold`** — generates the `git mesh add` / `git mesh why` / `git mesh commit` commands for all fragment links not yet covered by a mesh.

## wiki check --mesh

```bash
wiki check --mesh
wiki check --mesh wiki/architecture/*.md
wiki check --mesh "packages/auth/**/*.wiki.md"
```

Extends the existing `wiki check` validation pass with a mesh coverage check. For each internal fragment link with a line range, it runs `git mesh ls <path>#L<s>-L<e> --porcelain` and verifies that at least one returned mesh also anchors the wiki file containing the link. Any uncovered link is reported as an error (non-zero exit).

The `--mesh` flag is opt-in so that repositories without meshes are unaffected by default. Glob targeting follows the same rules as bare `wiki check`: omitting globs defaults to `$WIKI_DIR/**/*.md` plus `**/*.wiki.md` (with `$WIKI_DIR` defaulting to `wiki`).

## wiki mesh scaffold

```bash
wiki mesh scaffold
wiki mesh scaffold wiki/architecture/*.md
wiki mesh scaffold "packages/auth/**/*.wiki.md"
```

Scans the same file set as `wiki check` and emits shell commands to create meshes for every fragment link not yet covered. Output is printed to stdout — nothing is staged or committed.

For each uncovered link the scaffold emits:

```bash
git mesh add wiki/<page-title-slug>/<target-slug> \
  <wiki-file> \
  <path>#L<start>-L<end>
git mesh why wiki/<page-title-slug>/<target-slug> \
  -m "<why derived from surrounding prose>"
```

### Mesh naming

Names follow the `wiki/<page-title-slug>/<target-slug>` convention:

- **Page title slug** — derived from the wiki page's frontmatter `title` field (falling back to the filename stem). This keeps names stable across file renames.
- **Target slug** — derived from the link label (truncated at five words, falling back to the target file stem for long or path-style labels).

Names are topical, not path-derived: one wiki page will typically produce several meshes covering different subsystems. Authors are expected to rename generated slugs to match the conceptual relationship before committing.

### Why generation

The `why` is extracted from the prose sentence containing the link, with all markdown syntax stripped. This produces a first-draft definition of the subsystem the anchors collectively form. Per the git mesh handbook:

> Write the **why** as a definition: name the subsystem the anchors collectively form and say plainly what it does across them.

Generated whys require author review — sentences that started with a backtick identifier produce headless predicates, and bullet-list summary lines produce terse fragments. The scaffold inserts the link label as a reconstructed subject when it detects a headless verb.

### Default glob behavior

Omitting globs is equivalent to:

```bash
wiki mesh scaffold "$WIKI_DIR/**/*.md" "**/*.wiki.md"
```

where `$WIKI_DIR` defaults to `wiki`. This matches the default discovery behavior used by all other wiki commands (see [discover_files](packages/cli/src/commands/mod.rs#L141-L183&9b91dfb)).

## Workflow

```bash
# 1. Generate scaffold for all uncovered links
wiki mesh scaffold > meshes.sh

# 2. Review and edit mesh names and whys in meshes.sh

# 3. Run the scaffold
sh meshes.sh

# 4. Commit each mesh
git mesh commit wiki/<name>

# 5. Validate coverage
wiki check --mesh
```

## References

- [discover_files](packages/cli/src/commands/mod.rs#L141-L183&9b91dfb)
