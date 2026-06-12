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

## Merge conflict resolution

`wiki check --fix` also resolves `.wiki/` merge conflicts automatically by consuming the git-mesh-core `merge_mesh_files()` kernel. After a merge that produces conflict markers in a `.wiki/` mesh file, running `--fix` collapses the markers: anchor lines are compared against the worktree source and kept or removed so the mesh reflects whichever side survived the source merge.

**Clean-source precondition:** The source file (the code file the anchor targets) must be free of its own conflict markers. `--fix` reads the worktree file as-is to produce the correct anchor hash. If the file still has markers, the mesh is left untouched and the report names the file you must resolve first.

**Diverged `--why` rationale:** If both sides changed the rationale text differently, anchor lines resolve cleanly but the `--why` line retains `<<<<<<<` / `>>>>>>>` residue markers. This is safe to commit only after manual reconciliation — a mesh with residue is **not** re-staged.

**Relation to the merge driver:** For an even smoother experience, see `./git-hook-setup.md` for the optional `wiki mesh merge` git merge driver. It collapses easy `.wiki/` conflicts mid-merge so they never surface as markers. Clones that skip setup fall back to git's line merge + `--fix` — no loss of correctness.
