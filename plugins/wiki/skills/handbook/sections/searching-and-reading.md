# Searching and reading

The everyday entry point. `wiki [query]` is the default subcommand — no verb needed.

```bash
wiki "auth policy"        # ranked titles + summaries
wiki -l 10 "auth"         # up to 10 results (default 3)
wiki -l 10 -o 10 "auth"   # next page
```

Search first, before authoring. It surfaces the canonical title to link against and exposes a collision before you create one. An exact title still works as a query, but the output is ranked lookup, not a direct fetch.

## Orientation

```bash
wiki list                 # every page: title, aliases, tags, path
wiki list --tag security  # filter
wiki summary "Authorization"   # one-line summary; resolves by title, alias, or path
```

`summary` reads from stdin when the argument is omitted, and emits `{ title, file, summary }` under `--format json`.

## Is this file even a wiki page?

A `.md` file is a wiki page **iff** its frontmatter has both a non-empty `title` and a non-empty `summary`. Everything else is plain markdown — invisible to search, but still flagged by `wiki check` if it sits under the checked tree without those fields. Don't assume a file in `wiki/` is a page; assume nothing, check the frontmatter.
