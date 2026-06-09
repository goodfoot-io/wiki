# Searching and reading

The everyday entry point. `wiki [query]` is the default subcommand — no verb needed.

```bash
wiki "auth policy"        # ranked titles + summaries
wiki -l 10 "auth"         # up to 10 results (default 3)
wiki -l 10 -o 10 "auth"   # next page
```

Search first when authoring — it surfaces the canonical title to link against. An exact title still works as a query, but the output is ranked lookup, not a direct fetch. A query that collides with a subcommand name (`mesh`, `check`, `list`) dispatches to that subcommand instead of searching.

## Orientation

```bash
wiki list                 # every page: title, aliases, tags, path
wiki list --tag security  # filter
wiki summary "Authorization"   # one-line summary; resolves by title, alias, or path
```

`summary` reads from stdin when the argument is omitted, and emits `{ title, file, summary }` under `--format json`.

## Is this file even a wiki page?

A `.md` file is a wiki page **iff** its frontmatter has both a non-empty `title` and a non-empty `summary`. A file missing either is plain markdown — invisible to search and silently skipped by `wiki check` (not flagged as broken; it's simply not a page). Don't assume a file in `wiki/` is a page; check the frontmatter.
