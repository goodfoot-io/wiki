# Organizing a wiki

Use this when starting a wiki, placing a new page, or deciding whether to restructure. The organizing rules are decisions, not aesthetics — apply the relevant one and move on.

## Does this content belong in the wiki at all?

The wiki's defining property: fragment links anchor prose to source, so drift is detectable. That property is the inclusion test — a narrower bar than most wikis, on purpose. A wiki that includes everything becomes a graveyard where stale pages outnumber live ones and search returns noise.

**In the wiki** when content can be anchored to source files, synthesizes *across* files/packages/subsystems, answers *why* or *how it connects* (the code answers *what*), and stays relevant across many commits.

**Elsewhere** when it can't be anchored (essays, external refs, philosophy → keep as plain markdown), describes a single function/file (→ JSDoc or package README, next to the code), or is ephemeral (completion checklists, session reports, one-time runbooks → don't wiki them).

## Where does a page live: centralize or embed?

Membership is frontmatter, not location — any `.md` anywhere with `title`+`summary` is a page. So place by *audience*, not by convenience.

- **Centralize in `wiki/`** when content is cross-cutting: it spans packages, or a reader who doesn't know which package to look in would need it. It has no natural home in the source tree.
- **Embed beside the component** (e.g. `packages/cards/DESIGN.md` with frontmatter) when content is about *one* component — its design, constraints, internals. Co-located docs are found and maintained by whoever changes the code, so they drift less. Frontmatter still registers them in the index, so embedding ≠ hiding.

## Keep the modes separate (Diátaxis)

Mixing documentation modes is the top source of rot — a page that is both reference and guide is optimized for neither, and the two halves go stale at different rates. Four modes; this wiki uses three:

| Mode | Purpose | Here |
|---|---|---|
| Explanation | why it works this way | `wiki/architecture/` |
| How-to | accomplish a specific task | `wiki/guides/` |
| Reference | look it up and leave | `wiki/meta/` |
| Tutorial | learning by guided experience | *omitted* — code, JSDoc, READMEs serve this better |

In an embedded page covering one small component, hold the separation with H2s (`## Explanation`, `## Guide`, `## Reference`). For a larger component, split by mode into separate files (`logging-design.md` vs `logging-api.md`).

## When to reorganize

Structure follows content patterns; it doesn't precede them. Reorganize only when you can **name a concrete failure** — a reader failing to find something, or finding the wrong thing. If it merely looks untidy but readers cope, leave it.

| Signal | Action |
|---|---|
| `wiki [query]` returns partial matches scattered across disconnected pages | concept is fragmented → merge, or add a hub |
| A new page's home is genuinely ambiguous between two sections | boundaries unclear → add a hub or sharpen the distinction |
| One page has grown to cover two independently-searched topics | split it |
| A subdirectory holds 3+ pages with no overview | add a hub page |

## Hub pages

A hub covers a domain broadly and wikilinks to its leaf pages; it holds no deep implementation detail. Hubs earn their place only once a subdirectory has enough leaves that a reader needs orientation first — before that, the directory's implicit identity (`wiki/architecture/` = architecture) suffices, and an early hub is just another page to keep current for little benefit. Where a hub exists, make it the canonical entry point: other pages link to the hub, not its leaves, unless they need a specific leaf.
