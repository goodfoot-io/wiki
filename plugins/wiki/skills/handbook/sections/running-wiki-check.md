# Running `wiki check`

The health gate. Validates links, frontmatter, and mesh coverage in one pass.

```bash
cd wiki && wiki check     # scope to wiki/; the usual invocation
wiki check                # every *.md under CWD
wiki check path/to/page.md  # specific globs, resolved from CWD
```

## Three diagnostic buckets

| Bucket | Examples | Fix |
|---|---|---|
| Frontmatter / link errors | missing `title`/`summary`, title collision, broken link, dangling `#heading` anchor | plain-text edit in the page |
| Drifted anchor (line range) | cited bytes changed | `--fix` re-anchors if unambiguous; else skips → `./resolving-skipped-fixes.md` |
| `mesh_uncovered` | fragment link has no covering mesh | `--fix` creates it → `./fixing-mesh-coverage.md` |

## Scoping is by CWD, resolution is by repo root

Bare `wiki check` validates every page beneath the CWD; explicit globs resolve from CWD. But link, anchor, `git check-ignore`, and mesh resolution stay anchored at the **git repo root** — so a subdirectory check yields the same diagnostics as the equivalent repo-relative glob from root. `cd` changes *what is selected*, not *how anchors resolve*.

## Useful flags

```bash
wiki check --no-exit-code   # report-only; exits 0 even with errors
wiki check --format json    # structured diagnostics (for scripts)
wiki --source index check   # validate staged content (pre-commit); also: worktree (default), head (CI)
```

`--source` reads a different repo snapshot without touching the worktree. `--fix` requires `--source=worktree` because it rewrites files on disk.

## Merge conflicts in `.wiki/` meshes

Conflict-markered `.wiki/` mesh files are reported as errors in read-only `wiki check` (without `--fix`). The same errors are **resolvable** by running with `--fix`, which consumes the git-mesh-core `merge_mesh_files()` kernel to collapse the markers automatically. See `./fixing-mesh-coverage.md` for details and the two fail-closed cases (conflicted source file, diverged `--why` rationale).
