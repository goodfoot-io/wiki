---
title: Organizing Wiki Content
summary: Whether content belongs in the wiki, where pages live (centralize vs embed), mode separation, and when to reorganize.
tags: [wiki, how-to]
---

Decisions, not aesthetics — apply the relevant one and move on.

## Does this belong in the wiki at all?

The wiki's defining property: fragment links anchor prose to source, so drift is detectable. That is the inclusion test — deliberately narrower than most wikis, because a wiki that includes everything becomes a graveyard where stale pages outnumber live ones.

- **In**: anchorable to source files; synthesizes across files/packages/subsystems; answers *why* or *how it connects* (the code answers *what*); relevant across many commits.
- **Elsewhere**: unanchorable essays → plain markdown; single-function/file detail → JSDoc or package README next to the code; ephemera (checklists, session reports) → nowhere.

## Where does a page live?

Membership is frontmatter, not location — any `.md` anywhere with `title`+`summary` is a page. Place by audience:

- **Centralize in `wiki/`** for cross-cutting content spanning packages, or anything a reader who doesn't know which package to look in would need.
- **Embed beside the component** (`packages/x/DESIGN.md` with frontmatter) for one component's design, constraints, internals — co-located docs are maintained by whoever changes the code. Frontmatter registers them in search either way: embedding ≠ hiding.

## Keep the modes separate

| Mode | Purpose | Here |
|---|---|---|
| Explanation | why it works this way | `wiki/architecture/` |
| How-to | accomplish a task | `wiki/guides/` |
| Reference | look it up and leave | `wiki/meta/`, `wiki/reference/` |

(Tutorial is omitted — code, JSDoc, READMEs serve it better.) Mixing modes is the top source of rot: the halves go stale at different rates. In a small embedded page, hold the separation with H2s; in a large one, split by mode into separate files.

## When to reorganize

Reorganize only on a named failure — a reader failing to find something, or finding the wrong thing.

| Signal | Action |
|---|---|
| One query returns partial matches scattered across pages | concept fragmented → merge or add a hub |
| A new page's home is genuinely ambiguous | boundaries unclear → add a hub or sharpen distinctions |
| One page covers two independently-searched topics | split it |
| A subdirectory holds 3+ pages with no overview | add a hub |

## Hubs

A hub covers a domain broadly and links to its leaves; it holds no deep detail. Add one only once leaves exist that need orientation first — an early hub is just another page to keep current. Where a hub exists it is the canonical entry point: link to the hub, not its leaves, unless a specific leaf is needed.
