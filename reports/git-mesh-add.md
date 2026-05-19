# Bug report: `git mesh add` prefix-collision guard is HEAD-based, not index/worktree-aware

- **Component:** git-mesh
- **Affected version:** `git-mesh 1.0.81`
- **Reported:** 2026-05-19
- **Status:** **Resolved in git-mesh 1.0.83.** The collision guard now consults
  the index/working tree: with a two-step rename applied but uncommitted,
  `git mesh add <child>` succeeds (exit 0). Verified with the reproduction
  below on `git-mesh 1.0.83`. The sections below describe the failure as
  observed on **1.0.81** and are retained for history.
- **Severity (on 1.0.81):** High for tooling that resolves prefix collisions
  inside a pre-commit hook (e.g. `wiki scaffold`), which mutates but does not
  commit.

## Summary

`git mesh add <name>` rejects a name when an existing mesh is a strict path
prefix of it (file-vs-directory collision). The remediation git-mesh itself
suggests is to rename the blocking mesh. On 1.0.81 the collision guard was
evaluated against the **committed HEAD tree only**: a rename applied to the
working tree *and* the index — but not yet committed — did **not** clear the
collision, so `git mesh add` still failed, citing the old (pre-rename) name
that no longer existed on disk or in the index.

This made the documented remediation unusable from any workflow that resolves
the collision and creates the dependent mesh in the same uncommitted step
(pre-commit hooks, scaffolders, batch fixers).

## Expected behavior

After a mesh is renamed so the colliding path is free in the working tree and
index, `git mesh add <child-name>` should succeed within the same uncommitted
working state — consistent with git-mesh's own guidance to "rename one of them
… and retry." (This is the behavior on 1.0.83.)

## Actual behavior (1.0.81)

`git mesh add <child-name>` continued to fail with the original collision
error, naming the **old** mesh, until the rename was committed to HEAD.

## Reproduction

```bash
T=$(mktemp -d); cd "$T"
git init -q
git config user.email t@t.t; git config user.name t
printf 'l1\nl2\nl3\nl4\nl5\n' > code.txt
printf 'l1\nl2\nl3\nl4\nl5\n' > doc.md
git add -A; git commit -qm init

# Blocker mesh occupying path `a/b` as a file, committed into HEAD.
git mesh add a/b code.txt#L1-L2 doc.md#L1-L2
git mesh why a/b -m "blocker mesh occupying path a/b as a file"
git add .mesh; git commit -qm "add mesh a/b"

# Baseline: child add collides (expected on all versions).
git mesh add a/b/c code.txt#L3-L4 doc.md#L3-L4
#   -> exit 1: "mesh name `a/b/c` collides with existing mesh `a/b`"

# git-mesh's own suggested fix, direct form — IMPOSSIBLE (all versions):
git mesh move a/b a/b/index
#   -> exit 1: "git: create .../.mesh/a/b: File exists (os error 17)"
#      (cannot turn the file `a/b` into directory `a/b/` in one move)

# Two-step rename via a temp name (temp must be kebab-case; `_` is rejected):
git mesh move a/b tmp-blocker-hold       # exit 0
git mesh move tmp-blocker-hold a/b/index # exit 0
# Working tree + index now reflect the rename:
#   find .mesh -type f      -> .mesh/a/b/index
#   git status --porcelain  ->  D .mesh/a/b   (old path gone)

# Retry the child add with the rename applied but NOT committed:
git mesh add a/b/c code.txt#L3-L4 doc.md#L3-L4
#   1.0.81 -> exit 1: STILL "collides with existing mesh `a/b`"
#   1.0.83 -> exit 0: "Added 2 anchors to mesh `a/b/c`."   (FIXED)
```

## Evidence

### 1.0.81 — child add blocked while rename uncommitted

```
fs after two-step:
.mesh/a/b/index
git status .mesh (uncommitted):
 D .mesh/a/b

=== uncommitted: git mesh add a/b/c ===
git mesh add: cannot add mesh `a/b/c`
mesh name `a/b/c` collides with existing mesh `a/b`: loose refs cannot occupy
the same path as both a file and a directory.
add(uncommitted) exit=1
```

Child add succeeded only after committing the rename to HEAD (`exit 0`).

### 1.0.83 — child add succeeds while rename uncommitted (fix verified)

```
git-mesh 1.0.83
baseline add exit=1 (collision, expected)
move1 exit=0
move2 exit=0
.mesh/a/b/index
 D .mesh/a/b
--- child add with rename uncommitted ---
Added 2 anchors to mesh `a/b/c`.
add(uncommitted) exit=0
```

## Additional findings (still apply on 1.0.83)

1. **The suggested one-step fix is impossible.** git-mesh's error text suggests
   `git mesh move a/b a/b/index`, but that fails with
   `create .../.mesh/a/b: File exists (os error 17)` because `a/b` is a regular
   file and the destination needs `a/b/` as a directory. A two-step rename
   through an intermediate top-level name is required.
2. **Temp/intermediate names must be kebab-case.** `git mesh move a/b zzz_tmp`
   fails: `segment 'zzz_tmp' contains invalid character '_'`. Any intermediate
   name used by tooling must use only `[a-z0-9-]` segments.

## Resolution / remaining recommendations

Resolved by upgrading to **git-mesh 1.0.83**, whose collision guard consults
the index/working tree. `wiki scaffold`'s collision-rename remedy now converges
in a single uncommitted pre-commit run.

Recommended follow-ups for git-mesh (not blocking):

- Make `git mesh move <file-name> <file-name>/<leaf>` perform the file→directory
  transition internally (via a temp) so the one-step command printed in the
  error message actually works.
- State the kebab-case constraint on the destination in the move error text.
