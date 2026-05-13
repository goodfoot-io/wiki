---
name: wiki
description: You must load this if the user mentions wikis
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

Fragment links anchor prose to specific code locations and serve as documentation coverage markers. **This only works if a link exists.** A file referenced in prose but not linked is a blind spot: the documentation can drift as the code evolves without any signal.

**Every source file the documentation relies on MUST have at least one fragment link.** This includes files whose types, constants, schemas, or behaviours are described — not only files whose functions are explained in detail.

```markdown
The [rebuild function](packages/cards/src/rebuild.ts#L15-L45) re-indexes all card files
by walking the card directory and upserting rows into the [card_files table](packages/cards/src/schema.ts#L8-L22).
```

Heuristics:
- Target whole function or struct definitions (signature through closing brace)
- Paths are resolved relative to the linking file's directory (use `../` to climb out of `wiki/`)
- Backticks in the link label (e.g. `` [`fn`](path) ``) are supported
- Include broad context — a link that goes stale when surrounding code changes is working as intended

When a file is relied upon but cannot be worked naturally into prose, add a **References** section at the bottom:

```markdown
## References

- [rebuildCardFiles](packages/cards/src/rebuild.ts#L15-L45)
- [card_files schema](packages/cards/src/schema.ts#L8-L22)
- [CardFile type](packages/cards/src/types.ts#L3-L12)
```

## Page-to-page links

Link to related wiki pages using standard markdown relative-path syntax (`[Title](./other-page.md)` or `[Title](./other-page.md#heading)`), resolved against the linking file's directory. When creating companion pages (e.g., an architecture reference and a maintenance guide), add bidirectional links between them.

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
- **Multiple pages touch the topic**: Determine the natural home; update it, add relative markdown links from the others
- **Existing non-wiki document to convert**:
  - **External project copy or resolved bug/incident report**: exclude — not durable knowledge about this codebase
  - **Notes, working plans, confirmed decisions**: add `title` + `summary` frontmatter following `<page-format>` — existing prose can stay as-is

### Location and Filename (new pages)

Place all wiki pages under the `wiki/` directory tree. Use the kebab-cased slug of the title as the filename.

```
wiki/
  meta/           # wiki conventions, CLI docs
  architecture/   # system design, data models
  guides/         # workflows, how-tos
  ...             # new subdirectories as needed
```

## 3. Organize

### Page Types

- **Hub pages** — link to sub-pages via relative markdown links (`[Card Files Table](./card-files-table.md)`); always in `wiki/`
- **Leaf pages** — cover one concept with fragment links to all relevant code
- **Long-form pages** — complex workflows as a single page with sections

### When to Reorganize

Act on these signals:
- **`wiki [query]` returns multiple partial matches for one concept** — fragmentation: merge pages or add a hub
- **Location of a new page is ambiguous between two sections** — taxonomy failure: clarify section scope or add a hub
- **A page covers two topics each worth searching for independently** — bloat: split the page
- **A subdirectory has 3+ pages with no overview** — add a hub page

### How to Reorganize

- **Merge**: combine two pages into one; update relative markdown links from the removed page to the merged one
- **Split**: divide one page into two; add bidirectional relative markdown links between them
- **Add hub**: create an overview page in the subdirectory that links to its pages via relative markdown links

## 4. Update Related Pages

After creating, editing, or reorganizing, search for pages that should cross-reference the changed content:

```bash
wiki "card files rebuild"
```

Read each match and add a relative markdown link where relevant. If a related page discusses components now better covered by the changed page, add cross-references rather than duplicating content. This applies even to brand-new pages — existing pages may mention related concepts without linking to them.

## 5. Pin and Validate

Validate only the files you changed:

```bash
wiki check --fix "wiki/architecture/my-page.md"
wiki check --fix "wiki/**/*.md"
```

Use `wiki check --root wiki --fix` only when changes span many pages. `check --fix` pins unpinned links and validates in a single pass — do not run `wiki check` separately afterward.

For the full maintenance workflow (stale link triage, prose updates, backlink propagation, commit), see the maintenance reference bundled with this skill.

## 6. Update Wiki Feedback

Update [Wiki CLI Feedback](../../../wiki/meta/wiki-feedback.md) with any friction, bugs, or feature requests observed:

```bash
wiki summary "Wiki CLI Feedback" # confirm the canonical page before editing it
# then edit wiki/meta/wiki-feedback.md
```

Add entries under **Feature Requests**, **Bug Reports**, or **Observations**. Keep entries concise — describe observed behavior and its impact, not solutions.

</instructions>
