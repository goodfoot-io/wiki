---
title: Wiki Git Hook Setup
summary: Wiring wiki check --fix into pre-commit — one best-effort pass that stages exactly the files it rewrote, and never blocks a commit.
tags: [wiki, how-to]
---

One pre-commit concern, one invocation: `wiki check --fix` repairs mechanical drift in the working tree, then the hook stages **exactly** the files that run rewrote. It is a local guard, not a commit gate: unresolvable errors print to stderr but never block (`--no-exit-code`), and a missing `wiki` binary passes silently.

## The hook

Copyable script: [`./examples/pre-commit.wiki.sh`](../examples/pre-commit.wiki.sh) — one line does the work:

```bash
APPLIED=$("$WIKI_BIN" check --fix --print-applied --no-exit-code --source=worktree)
```

then stages each printed path. Why each flag is load-bearing:

- **`--print-applied`** — machine-readable list of rewritten files on stdout; summary and diagnostics go to stderr. Staging this list exactly means no unrelated dirty `.md` file is ever swept into the commit (an early hook version staged a blanket `git diff` and did exactly that — a hook defect, not a CLI defect).
- **`--fix`** — repair before review; worktree-only by nature.
- **`--no-exit-code`** — repair-and-continue; the hook never rejects a commit.

## Wiring

One-time per clone: copy [`./examples/pre-commit.wiki.sh`](../examples/pre-commit.wiki.sh) to your `core.hooksPath` as `pre-commit`, make it executable.

## Whole corpus, not just staged files

A staged edit can break a link on an *unstaged* page or collide with an untouched page's title; checking everything catches cross-page failures. The pass is idempotent and cache-backed, so it's cheap per commit.

Preview before wiring: `wiki check --fix --fix-dry-run`.
