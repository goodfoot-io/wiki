# Running `wiki check`

The health gate. Validates links and frontmatter, and classifies every line-range link against its git-derived anchor epoch in one pass.

```bash
cd wiki && wiki check --fix  # scope to wiki/; the usual invocation — clears mechanical drift first
wiki check                   # read-only; every *.md under CWD
wiki check path/to/page.md   # specific globs, resolved from CWD
```

## Three diagnostic buckets

| Bucket | Examples | Fix |
|---|---|---|
| Frontmatter / link errors | missing `title`/`summary`, title collision, broken link, dangling `#heading` anchor | plain-text edit in the page |
| Drifted anchor (line range) | cited bytes changed since the anchor epoch | `--fix` relocates the link when the content moved; in-place drift skips → `./resolving-skipped-fixes.md` |
| Uncertified / unverifiable | page has no `links-reviewed:` field, or the certified content occurs at multiple locations | fail-closed: re-anchor the link and bump `links-reviewed:` → `./fragment-links-and-coverage.md` |

## Scoping is by CWD, resolution is by repo root

Bare `wiki check` validates every page beneath the CWD; explicit globs resolve from CWD. But link, anchor, and `git check-ignore` resolution stay anchored at the **git repo root** — so a subdirectory check yields the same diagnostics as the equivalent repo-relative glob from root. `cd` changes *what is selected*, not *how anchors resolve*.

## Useful flags

```bash
wiki check --no-exit-code   # report-only; exits 0 even with errors
wiki check --format json    # structured diagnostics (for scripts)
wiki --source index check   # validate staged content (pre-commit); also: worktree (default), head (CI)
```

`--source` reads a different repo snapshot without touching the worktree. `--fix` requires `--source=worktree` because it rewrites files on disk. `--fix-dry-run` previews what `--fix` would rewrite without mutating anything; `--print-applied` prints the repo-relative path of each file the run rewrote to stdout, one per line, so callers can stage exactly what the run touched.

## Shallow clones fail closed

The anchor-epoch lookup walks the page's full commit history. In a shallow clone the check reports those links as unverifiable rather than guessing — clone with full history (CI: `fetch-depth: 0`) wherever the check runs.
