---
title: Search Commands And Output Shapes
summary: Search, list, and summary — ranking, output shapes, title/alias/path resolution, and verb-dispatch vs search fallback.
tags: [wiki, reference]
links-reviewed: 2
---

`wiki [query]` is the default subcommand — no verb needed.

```bash
wiki "auth policy"        # ranked results (default 3)
wiki -l 10 -o 10 "auth"   # page through
wiki list                 # corpus: title, aliases, tags, path
wiki list --tag security  # tag filter
wiki summary "Authorization"    # one-line summary by title, alias, or path
echo wiki/page.md | wiki summary   # stdin when argument omitted
```

## Ranking

Four stages in priority order: exact title match → exact alias-token match → path fragment (only when the query contains `/`) → BM25 full-text over weighted fields — title 5, aliases 4, tags 3, keywords 3, summary 2, body 1 ([weights](/packages/cli/src/index/search.rs#L216-L217)). Query tokens are prefix-matched; snippets come from body text. Zero matches prints nothing (human) or `[]` (JSON) and exits 0.

## Verb dispatch vs search fallback

A first token equal to `check`, `list`, or `summary` dispatches to that subcommand; anything else is a search query. Topic-word queries (`wiki drift`, `wiki stale`, `wiki reconciliation`) are searches for pages about those topics — they exit 0 whether or not they hit.

## Output shapes

- Human search/summary: `# Title`, `## repo-relative/path`, summary.
- `--format json`: search → `[{title, file, summary, snippets:[{line,text}]}]`; summary → `{title, file, summary}`; list → `[{title, aliases, tags, summary, file}]`.
- Path quirk: **search and summary JSON emit absolute paths; list emits repo-relative.** Human output is always repo-relative.
- Alias hits print `(alias of <Canonical Title>)`.

## Resolution order (`wiki summary X`)

1. Exact title, case-insensitive.
2. Exact alias token, case-insensitive.
3. Path — only if `X` contains `/` or ends `.md`; case-**sensitive**, substring-capable.

Not found → suggestions to stderr, exit 1. Infrastructure failure → exit 2.

## Is this file even a wiki page?

A `.md` file is a wiki page iff its frontmatter has both a non-empty `title` and a non-empty `summary`. Anything else is plain markdown — invisible to search, silently skipped by `wiki check`. Don't assume a file under `wiki/` is a page; check the frontmatter.
