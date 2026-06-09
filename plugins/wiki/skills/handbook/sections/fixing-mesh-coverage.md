# Fixing mesh coverage

`mesh_uncovered` means a line-ranged fragment link has no mesh anchoring both its page and its code target (the contract in `./fragment-links-and-coverage.md`). This is the most common remediation, and it is almost always automatic.

## Let `--fix` do it

```bash
wiki check --fix --fix-dry-run   # preview meshes to be created — no mutation
wiki check --fix                 # create them (anchors only, no why); requires --source=worktree
```

`--fix` walks the whole corpus, builds its own coverage index, and creates a mesh for every uncovered fragment link. Idempotent — already-covered links are skipped — so it is safe on every commit.

## The hook already runs it

The pre-commit hook runs `wiki check --fix --print-applied --no-exit-code --source=worktree` and stages exactly what it touched. So the normal flow is just:

```bash
git commit -m "wiki: ..."   # hook creates + stages meshes and re-stages auto-fixed pages
```

`--print-applied` prints one repo-relative path per created/renamed mesh to **stdout** (advisories → stderr), letting the hook `git add` exactly those paths instead of a blanket `git add .wiki/`. It conflicts with `--fix-dry-run`.

## Slug collisions

When a new slug path-collides with a pre-existing ancestor mesh, `--fix` renames the blocker to `<blocker>/<derived-leaf>` (or `<blocker>/index`) so both coexist, prints the renamed path for staging, and notes it on stderr. Not an error — preview it with `--fix-dry-run`.

## When `--fix` can't

`--fix` is best-effort and fail-closes on what it can't safely resolve: a missing target path, an ambiguous line-range shift, deleted or moved content, a slug it can't create. It emits a **skip line naming the exact `wiki mesh` command to run**. That line is your starting point — go to `./resolving-skipped-fixes.md`.

To curate (add/revise a `why`) on an existing mesh:

```bash
wiki mesh add <slug> --why "What this mesh covers."   # mesh must already exist
git add .wiki/
```
