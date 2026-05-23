# Bug report: `git mesh stale` reports a gitignored anchor target as `deleted`, with no way to distinguish "permanently uncommittable" from "not committed yet"

- **Component:** git-mesh
- **Affected version:** `git-mesh 1.0.85`
- **Reported:** 2026-05-22
- **Status:** **Resolved in git-mesh 1.0.86** via recommendation (1): `git mesh
  add` now refuses a gitignored anchor path (`anchor path is gitignored: …`,
  exit 1) while still accepting untracked-but-not-ignored targets (exit 0).
  Verified with the reproductions below on 1.0.86. The sections below describe
  the failure as observed on **1.0.85** and are retained for history.
- **Severity:** High for any repo that anchors a generated/gitignored file —
  `git mesh stale` is **permanently non-zero** with no in-tool resolution,
  which breaks CI gating on `stale`.

## Summary

`git mesh add` accepts an anchor whose target path is **gitignored** (a build
artifact that is present on disk but never enters git). `git mesh stale` then
resolves that anchor through git's layers, finds the path in none of them, and
reports it as `deleted` — exit 1 — **even though the file exists on disk**.

The deeper problem is that git-mesh cannot distinguish two very different
situations, reporting both identically as `deleted`:

1. **gitignored** — the path is excluded by `.gitignore` and will *never* be
   committed (a generated file, a build artifact). The anchor is **permanently**
   unresolvable for everyone; no commit fixes it.
2. **untracked-but-not-ignored** — a brand-new source file the author simply
   hasn't `git add`ed yet. This is **transient**: the moment the file is
   committed, the anchor resolves and `stale` goes green on its own.

Case 2 self-heals and is arguably correct to flag (you cited a file you haven't
committed). Case 1 is a trap: it can never be cleared, and `git mesh add`
created the doomed anchor without complaint.

## Expected behavior

git-mesh should not leave the user in an unresolvable state because of a
gitignored anchor target. Either of these would suffice (preference order
below):

1. **Refuse at creation (preferred, fail-closed at the source).** `git mesh add`
   should reject an anchor path that is gitignored — analogous to how it already
   rejects absolute paths and `..` paths — with a clear message ("anchor path
   `generated.ts` is gitignored; git-mesh tracks content through git and cannot
   resolve a path git never sees"). Crucially this must key on **gitignored**,
   *not* "untracked": an untracked-but-not-ignored file is a legitimate anchor
   that will resolve on commit, so it must still be allowed.
2. **Distinguish in `stale`.** Report a gitignored anchor target with a distinct
   status (e.g. `ignored` rather than `deleted`) and a hint that the cause is a
   `.gitignore` match — and optionally let it be suppressed
   (`--ignore-untracked` / `--ignore-ignored`) so `stale` can be green. This is
   weaker than (1) because the meaningless anchor still exists; prefer it only
   if refusing at `add` time is undesirable.

Either way the gitignored/untracked distinction is the load-bearing part:
**permanent (gitignored) must be handled differently from transient
(untracked-not-ignored).**

## Actual behavior (1.0.85)

- `git mesh add ignored-demo … generated.ts#L1-L5` → **exit 0**, anchor created.
- `git mesh stale --worktree` → `generated.ts#L1-L5 — deleted`, **exit 1**, even
  though `generated.ts` is present on disk.
- No commit clears it (the file is gitignored, so it never enters HEAD/index).
- `git mesh remove`ing the anchor clears it, but any tool that re-derives
  coverage (e.g. a wiki scaffolder run from a pre-commit hook) re-adds it — see
  "Downstream impact" below.
- `git mesh stale --ignore-unavailable` does **not** help: that flag downgrades
  *unreadable* content to informational, not gitignored/untracked targets. It
  still prints `— deleted` and exits 1.

## Reproduction

Self-contained; only `git` and `git-mesh` required.

### Case A — gitignored target: permanently `deleted` (the bug)

```bash
T=$(mktemp -d); cd "$T"
git init -q
git config user.email t@t.t; git config user.name t; git config commit.gpgsign false
printf 'l1\nl2\nl3\nl4\nl5\n' > doc.md
git add doc.md; git commit -qm init

# A generated build artifact: present on disk, but gitignored.
printf 'gen1\ngen2\ngen3\ngen4\ngen5\n' > generated.ts
echo 'generated.ts' > .gitignore
git add .gitignore; git commit -qm "ignore generated.ts"

git check-ignore generated.ts          # -> generated.ts  (it IS ignored)
[ -f generated.ts ] && echo "on disk"  # -> on disk

# git mesh add accepts the gitignored anchor without complaint:
git mesh add ignored-demo doc.md#L1-L2 generated.ts#L1-L5   # exit 0
git add .mesh; git commit -qm "mesh anchoring gitignored file"

rm -f .git/mesh/stale-cache.db .git/mesh/file-index
git mesh stale --worktree               # -> generated.ts#L1-L5 — deleted ; exit 1
```

### Case B — untracked-but-not-ignored target: self-heals on commit (contrast)

```bash
T=$(mktemp -d); cd "$T"
git init -q
git config user.email t@t.t; git config user.name t; git config commit.gpgsign false
printf 'l1\nl2\nl3\nl4\nl5\n' > doc.md
git add doc.md; git commit -qm init

printf 'n1\nn2\nn3\n' > newcode.ts       # on disk, NOT committed, NOT ignored
git check-ignore newcode.ts || echo "(not ignored)"

git mesh add transient-demo doc.md#L3-L4 newcode.ts#L1-L3
git add .mesh; git commit -qm "mesh anchoring untracked-not-ignored file"

rm -f .git/mesh/stale-cache.db .git/mesh/file-index
git mesh stale --worktree                # -> newcode.ts#L1-L3 — deleted ; exit 1

git add newcode.ts; git commit -qm "commit newcode.ts"
rm -f .git/mesh/stale-cache.db .git/mesh/file-index
git mesh stale --worktree                # -> 0 stale across 1 mesh ; exit 0  (HEALED)
```

## Evidence (git-mesh 1.0.85)

### Case A — gitignored, present on disk, still `deleted`

```
on disk? yes; check-ignore -> generated.ts
Added 2 anchors to mesh `ignored-demo`.

- added: `ignored-demo` `doc.md#L1-L2`
- added: `ignored-demo` `generated.ts#L1-L5`
-- add exit: 0 --
-- stale --worktree (file IS on disk): --
## ignored-demo
- generated.ts#L1-L5 — deleted
```

### Case B — untracked-not-ignored heals after the source file is committed

```
check-ignore -> (not ignored)
-- stale BEFORE committing newcode.ts: --
## transient-demo
- newcode.ts#L1-L3 — deleted
exit: 1
-- stale AFTER committing newcode.ts: --
0 stale across 1 mesh (2 anchors checked)
exit: 0
```

The contrast is the whole point: **B clears itself; A never can.** git-mesh
reports both as `deleted`, so the two are indistinguishable to any caller (and
to CI).

## Downstream impact (why this is a hard deadlock, not just noise)

In our repo this surfaces through `wiki`, which derives mesh coverage from
line-ranged citations in documentation. When a wiki page cites a generated,
gitignored file with a line range:

1. `wiki check` demands mesh coverage for the citation (fail-closed).
2. `wiki scaffold` satisfies that by running `git mesh add` on the gitignored
   path — which, per Case A, git-mesh accepts.
3. `git mesh stale` then reports that anchor `deleted` forever.
4. `git mesh remove` clears it, but the next `wiki scaffold` (run by the
   fail-closed pre-commit hook) re-adds it, because the citation still demands
   coverage.

Net effect: a permanently red `git mesh stale` with no action that both clears
the finding and survives the hook. We are fixing the wiki side independently
(scaffold should not anchor gitignored targets; check should exempt them), but a
git-mesh-level guard would prevent any tool — or a hand-written anchor — from
falling into this trap in the first place. That is the motivation for the
preferred fix (1) above.

## Recommendations

1. **`git mesh add` refuses a gitignored anchor path** (keying on `.gitignore`
   match, *not* on tracked/untracked), with a message naming the gitignore
   cause. This enforces the invariant at the source: no mesh can ever anchor a
   path git cannot see, so `stale` can never go permanently-red from this cause.
2. **`git mesh stale` distinguishes gitignored from `deleted`** (distinct status
   + optional suppression flag) for cases where an anchor predates the guard, so
   the cause is diagnosable rather than silently conflated with a real deletion.
3. **Leave untracked-but-not-ignored anchors as-is** — flagging them as `deleted`
   until commit is acceptable (they self-heal), and silencing them would hide a
   genuine "you cited a file you haven't committed" signal.

## Resolution (git-mesh 1.0.86)

Recommendation (1) shipped. `git mesh add` now runs an anchor precheck that
refuses a gitignored path, keyed on the `.gitignore` match rather than on
tracked/untracked status:

```
=== CASE A: gitignored target — add now REFUSES ===
git mesh add: anchor precheck failed for `generated.ts`.

anchor path is gitignored: generated.ts
...
add exit: 1
-- meshes present? --
(none)

=== CASE B: untracked-not-ignored — add still SUCCEEDS ===
Added 2 anchors to mesh `transient-demo`.
add exit: 0
```

Because `add` now fails on a gitignored target, any tool that calls `git mesh
add` must pre-filter gitignored paths to avoid a (now legitimate) non-zero exit
— tracked on the wiki side (`wiki scaffold` skips gitignored targets; `wiki
check` exempts them from `mesh_uncovered`).

## Related

- `reports/git-mesh-add.md` — a prior git-mesh issue (HEAD-based prefix-collision
  guard) hit by the same `wiki scaffold` pre-commit workflow; resolved in 1.0.83.
