# Manual Testing Notes

Notes from a single linear pass through `wiki/guides/manual-testing-procedure.md`
against `wiki 0.5.37`. Focus is on behavior that diverges from the procedure
(non-spec) and behavior that was counter-intuitive or hard to use, including
mistakes I made on the first attempt.

## Bugs / non-spec behavior

### `wiki -n '*' list` is not supported (Step 5)

The procedure tells the reader to run:

```
wiki -n '*' list
```

and expects "pages from both namespaces, each row prefixed with its namespace."
The CLI rejects this:

> command does not support multi-namespace (`-n '*'`); supported: search
> (default query), check, links, summary, refs

Exit code is 2. Either `list` needs to be added to the multi-namespace allow-list,
or the procedure should drop this step. Notably, the error is also a useful
specification: `*` is supported on **search, check, links, summary, refs** —
not `list`, not `refs` for namespace prefixing in the source (see below), and
not `extract`/`html`/`serve`/`scaffold`/`hook`/`init`/`namespaces`.

### `wiki refs` strips/loses cross-namespace prefix (Step 8)

`docs/authentication.md` contains `[[scratch:Operator Notes]]`. The output of
`wiki refs "Authentication"` is:

```
## [[Sessions]] -> Sessions
/tmp/.../docs/sessions.md
tags: security
Session lifecycle.

## [[Operator Notes]] -> not found
```

Two problems:

1. The header re-prints the wikilink as `[[Operator Notes]]` — the `scratch:`
   prefix is lost in the display, which makes the diagnostic misleading.
2. The reference is reported as `not found`, even though `wiki -n scratch
   summary "Operator Notes"` resolves cleanly and `wiki check` passes on the
   same content. So `refs` is not consulting peer namespaces when resolving a
   `ns:Title` reference, while `check` is.

### `wiki links` does not surface cross-namespace backlinks (Step 9)

`notes/operator-notes.md` (namespace `scratch`) contains
`[[default:Authentication]]`. The procedure expects `wiki links "Authentication"`
to include `scratch:Operator Notes`. It does not — only same-namespace
backlinks (`Sessions`, `OAuth Notes`) appear. To surface cross-namespace
inbound links, the user has to know to pass `-n '*'` (which the procedure does
not mention for this step).

### `wiki html` deadlocks on the index lock (Step 13)

First invocation of `wiki html "Authentication"` failed with:

> timed out after 30s waiting for index lock at .../docs/.index.lock — another
> wiki process is holding it, or a previous run left it stuck

No other wiki process was running (`ps` confirmed). Removing the stale
`.index.lock` did **not** fix it — the next run timed out again with the same
error. The wiki log shows the command performs `index.prepare` twice in a
single invocation; the first prepare succeeds (~3 ms), then a second
`index.prepare` runs and times out at 30s. That looks like `html` re-entering
the lock-acquisition path within the same process while still holding the
lock, i.e. a self-deadlock.

Workaround: none found. `wiki serve` (Step 14) renders the same HTML over HTTP
without the deadlock, so the underlying renderer works — only the `html`
subcommand path is broken.

### `wiki install` flag-based, not subcommand-based, and only `--codex`/`--claude` (Step 16)

The procedure says: "subcommands listing supported integration targets (e.g.
`claude`, `gemini`). `wiki install <target>` writes the integration config".
Actual surface:

- It's flag-based: `wiki install --codex` and `wiki install --claude`.
- `--claude` is *informational only* — prints setup instructions without
  touching the filesystem or network.
- `gemini` is not a target.

### Stale `.index.lock` files left behind on timeout

When `wiki html` timed out, `.index.lock` files were left as zero-byte files
in both `docs/` and `notes/` (an unrelated namespace, even though `html` was
only invoked against `Authentication`). They had to be removed manually
before subsequent commands would proceed reliably. The error message tells
the user "a previous run left it stuck" but the CLI does not offer a flag to
clear it.

## Counter-intuitive behavior

### Parse-error row in `wiki namespaces` mislabels the namespace as `default` (Step 3c)

When `broken/wiki.toml` is unparseable, the row reads:

```
default	/tmp/.../broken	error: failed to parse .../broken/wiki.toml
```

Showing `default` in the namespace column for a file we couldn't parse is
misleading — it suggests the broken wiki has been silently registered as
another `default`. A literal `?` or `<unparsed>` in the namespace column
would be clearer.

### `wiki summary` and `wiki "<query>"` produce identical output for an exact title

Running `wiki summary "Authentication"` and `wiki "Authentication"` both print
title + path + summary; the search version adds matched snippets but otherwise
the headers are visually similar. It's not obvious from the output which
command was run, which makes copy-paste debugging harder.

### `-n '*'` allowed surface is not discoverable up front

The list of subcommands that accept `-n '*'` is only revealed by trying one
that doesn't (the error message enumerates them). `wiki --help` and the
per-subcommand help do not flag which subcommands are namespace-multi-aware.

### `wiki serve` binds `0.0.0.0`, not `127.0.0.1`

The startup line reads `Serving wiki on http://0.0.0.0:8765`. The procedure
hits `localhost`, which works, but the default bind address exposes the
server on all interfaces. Worth either documenting or defaulting to loopback.

## First-attempt mistakes

- I initially tried to follow Step 5's `wiki -n '*' list` literally and
  expected it to work — it does not. A reader who is dispositioned to "trust
  the procedure" will treat this as a regression rather than a doc bug.
- After `wiki html` deadlocked, I deleted `.index.lock` and retried, expecting
  the lock removal to be sufficient. It was not — the deadlock recurred,
  because the cause is internal to the `html` command, not a leftover from a
  prior process.

## Steps that worked exactly as documented

- Steps 0, 1, 2 (a–d), 3 (text + JSON + duplicate + parse-error exit codes), 4,
  5 (single + peer namespace), 6 (a–f), 7 (stdin/title/alias/missing),
  10 (`extract`), 11 (a–d clean/broken/cross-ns/JSON), 12 (`hook` clean and
  broken), 14 (`serve` index + page render), 15 (`scaffold` empty-output
  case), 17 (`WIKI_PERF=1` produces ~18 timing lines on stderr).
- Exit-code conventions matched the table: `0` for success, `1` for soft
  validation problems (duplicate namespaces, parse error, broken check), `2`
  for hard failures (no `wiki.toml`, unknown namespace, init refusal,
  index-lock timeout).
