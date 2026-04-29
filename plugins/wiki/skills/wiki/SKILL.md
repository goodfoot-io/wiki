---
name: wiki
description: Use `wiki mesh [query]` to search. You must load this for all other wiki usage.
---

<tools>
Run the `wiki` CLI for more information on usage.

```bash
wiki
```
</tools>

<page-format>

## Frontmatter

Every wiki page opens with YAML frontmatter:

```markdown
---
title: Card Files Table
summary: Card file storage schema, attachment handling, and rebuild logic for the card_files SQLite table.
tags:
  - database
  - cards
---
```

`title` is required and drives wiki resolution — it does not need to match the filename. `summary` is required; write agent-optimized summaries that include keywords, scope, and key components so an LLM scanning summaries can judge relevance without reading the full page. `tags` and `aliases` are optional.

## Content

**Synthesis over description.** Do not restate what the code says — explain what connects it, why it was designed this way, what tradeoffs were made, and what constraints apply.

## Fragment Links

Fragment links are the coverage mechanism. **Every source file the documentation relies on MUST have at least one fragment link.** A file referenced in prose but not linked is invisible to mesh coverage checks: the documentation will silently drift as the code evolves.

```markdown
The [rebuild function](packages/cards/src/rebuild.ts#L15-L45) re-indexes all card files
by walking the card directory and upserting rows into the [card_files table](packages/cards/src/schema.ts#L8-L22).
```

Heuristics:
- Target whole function or struct definitions (signature through closing brace)
- Paths MUST be repository-relative (e.g. `packages/foo.ts`, NOT `../../foo.ts`)
- `wiki check --fix` automatically converts absolute or file-relative paths to repo-relative
- Backticks in the link label (e.g. `` [`fn`](path) ``) are supported
- Include broad context — a link that goes stale when surrounding code changes is working as intended
- Do not add `@sha` manually — `wiki check --fix` pins unpinned links automatically
- Run `wiki check --mesh` to verify every fragment link with a line range is covered by a `git mesh`; use `wiki mesh scaffold` to generate missing meshes

When a file is relied upon but cannot be worked naturally into prose, add a **References** section at the bottom:

```markdown
## References

- [rebuildCardFiles](packages/cards/src/rebuild.ts#L15-L45)
- [card_files schema](packages/cards/src/schema.ts#L8-L22)
- [CardFile type](packages/cards/src/types.ts#L3-L12)
```

## Wikilinks

Link to related wiki pages using `[[Title]]` or `[[Title#Heading]]` syntax. Resolution is case-insensitive. When creating companion pages (e.g., an architecture reference and a maintenance guide), add bidirectional wikilinks between them.

</page-format>

<instructions>

## 1. Discover Relevant Pages

Search before writing or editing:

```bash
wiki "keyword"         # ranked default query; title matches score highest; exit 1 = no matches
wiki summary "Page Title" # print the canonical title, path, and summary for a known page
```

Search broadly — relevant content may appear in pages you wouldn't expect.

## 2. Decide and Write

Based on search results:
- **No relevant page exists**: Choose a location (see below), write following `<page-format>`
- **Page exists but lacks coverage**: Inspect it with `wiki summary "Page Title"` and the file on disk, then apply edits following `<page-format>`
- **Multiple pages touch the topic**: Determine the natural home; update it, add wikilinks from the others
- **Existing non-wiki document to convert**:
  - **External project copy or resolved bug/incident report**: exclude — not durable knowledge about this codebase
  - **Notes, working plans, confirmed decisions**: rename to `*.wiki.md`, prepend frontmatter following `<page-format>` — existing prose can stay as-is

### Location and Filename (new pages)

Choose based on scope and ownership:
- **Primarily about one component** — embed as `*.wiki.md` alongside it:
  - Covers design decisions, internal API, constraints, or rebuild logic for that component
  - Most fragment links point to code in the same package or directory
- **Cross-cutting or navigational** — place in `wiki/`:
  - Topic spans multiple packages, or is a workflow, how-to, or conceptual overview
  - Needs to be discoverable by someone unfamiliar with the codebase layout

```
wiki/
  meta/           # wiki conventions, CLI docs
  architecture/   # system design, data models
  guides/         # workflows, how-tos
  ...             # new subdirectories as needed
```

For `wiki/` pages, use the kebab-cased slug of the title as the filename. For embedded pages, use a descriptive name that fits the component directory (e.g. `DESIGN.wiki.md`, `schema.wiki.md`).

## 3. Organize

### Page Types

- **Hub pages** — link to sub-pages via wikilinks (`[[Card Files Table]]`); always in `wiki/`
- **Leaf pages** — cover one concept with fragment links to all relevant code
- **Long-form pages** — complex workflows as a single page with sections

### When to Reorganize

Act on these signals:
- **`wiki [query]` returns multiple partial matches for one concept** — fragmentation: merge pages or add a hub
- **Location of a new page is ambiguous between two sections** — taxonomy failure: clarify section scope or add a hub
- **A page covers two topics each worth searching for independently** — bloat: split the page
- **A subdirectory has 3+ pages with no overview** — add a hub page

### How to Reorganize

- **Merge**: combine two pages into one; update wikilinks from the removed page to the merged one
- **Split**: divide one page into two; add bidirectional wikilinks between them
- **Add hub**: create an overview page in the subdirectory that links to its pages via wikilinks
- **Move embedded → `wiki/`**: when a `*.wiki.md` page's fragment links have grown to span multiple packages
- **Move `wiki/` → embedded**: when a page's fragment links are all within one package

## 4. Update Related Pages

After creating, editing, or reorganizing, search for pages that should cross-reference the changed content:

```bash
wiki "card files rebuild"
```

Read each match and add a `[[wikilink]]` where relevant. If a related page discusses components now better covered by the changed page, add cross-references rather than duplicating content. This applies even to brand-new pages — existing pages may mention related concepts without linking to them.

## 5. Pin and Validate

Validate only the files you changed:

```bash
wiki check --fix "wiki/architecture/my-page.md"
wiki check --fix "documentation/**/*.wiki.md"
```

Use bare `wiki check --fix` only when changes span many pages. `check --fix` pins unpinned links and validates in a single pass — do not run `wiki check` separately afterward.

For the full maintenance workflow (stale link triage, prose updates, backlink propagation, commit), see the maintenance reference bundled with this skill.

</instructions>
