# Git Mesh Iteration Instructions

Use these instructions to continue the `packages/git-mesh` implementation loop toward `docs/git-mesh.md`.

## Goal

Bring `packages/git-mesh` into full practical alignment with `docs/git-mesh.md` by iterating in bounded, validated slices.

## Canonical Process

Repeat this exact loop:

1. Create a `gpt-5.4 low` general subagent to implement one bounded portion of the remaining work.
2. When the subagent returns:
   - review the diff yourself
   - run validation yourself
   - if there are warnings, errors, failing tests, or obvious implementation mistakes, do **not** fix them ad hoc unless the fix is trivial and clearly within the same slice; prefer dispatching another subagent to correct them
   - if the slice is good, commit it
3. Create a separate `gpt-5.4 low` general reviewer subagent to:
   - review `docs/git-mesh.md`
   - review `packages/git-mesh`
   - summarize the current state relative to the full spec
   - identify the next bounded slice
4. Dispatch the next implementation subagent using that reviewer recommendation.
5. Continue until the project is meaningfully complete or a real blocker appears.

The reviewer subagent is responsible for selecting the next slice. Do not freestyle the next slice if a reviewer result exists.

## Required Validation

After every code/config change slice, run all of these from `packages/git-mesh` unless the user explicitly says otherwise:

- `yarn lint`
- `yarn typecheck`
- `yarn test`

If the slice changes the wider workspace behavior in a meaningful way, also run:

- `yarn validate` from `/home/node/wiki`

Warnings are blocking.
Infrastructure failures are blocking.
Do not wave away failures as pre-existing.

## Commit Discipline

- Commit every validated slice before moving to the next review iteration.
- Use non-interactive git commands only.
- Never amend unless explicitly asked.
- Never reset or discard unrelated changes.
- If the agent/thread limit is reached, close completed subagents before spawning more.

## Current Working Rules

- Use `docs/git-mesh.md` as the north star.
- Prefer somewhat larger bounded slices when the remaining work clusters naturally.
- Keep implementation slices coherent and testable.
- Prefer maintaining momentum over over-planning.
- Preserve user changes and unrelated worktree changes.

## Current Project State

As of commit `1849110` (`Close git-mesh CLI parity gaps`), the project already has:

- v1 link storage under `refs/links/*`
- v1 mesh storage under `refs/meshes/*`
- mesh commit/update/reconcile/amend flows
- read-side inspection commands
- stale reporting with human, porcelain, JSON, JUnit, and GitHub Actions output
- culprit attribution and reconcile commands
- sync commands with lazy refspec bootstrap
- `doctor`
- CLI parity for several previously missing flags including:
  - `stale --patch`
  - `show --format=<fmt>`
  - `commit -F`
  - `commit --edit`
  - `commit --no-ignore-whitespace`

## Most Likely Remaining Gaps

Reviewer guidance before the latest slice said the biggest remaining gaps were shifting from major missing commands to last-mile parity and deeper algorithmic fidelity.

Expect the next reviewer to focus on one or more of:

- remaining spec mismatches in CLI output/details
- resolver fidelity versus true `git log -L` semantics
- stronger all-or-nothing write semantics around link creation plus mesh CAS
- merge/divergence workflow coverage
- any remaining flags or output contracts from `docs/git-mesh.md`

Do not assume this list is exhaustive. Use a fresh reviewer subagent every iteration.

## Recommended Reviewer Prompt

Use something close to this:

```text
Review /home/node/wiki/docs/git-mesh.md and /home/node/wiki/packages/git-mesh after the latest commits on the current branch. Describe the current state of the packages/git-mesh project relative to the functionality of the full specification in docs/git-mesh.md. Identify the next portion of work a subagent should perform.

Requirements:
- Focus on implementation state after the latest commit on the current branch.
- Propose the next slice that materially advances compliance with the spec.
- If the remaining work naturally clusters, prefer a somewhat larger bounded slice over an overly narrow one.
- Keep it self-contained and realistically implementable/testable in one subagent pass.
- Return:
  1. short current-state summary
  2. the recommended next slice
  3. why that slice is next
  4. likely files to change
```

## Recommended Implementation Prompt Shape

Use something close to this:

```text
Implement the next bounded git-mesh slice in /home/node/wiki/packages/git-mesh.

Goal:
- [paste reviewer-selected slice]

Required work:
- [paste reviewer-selected requirements]

Write scope:
- [paste likely files]

Constraints:
- Prefer coherent, testable behavior over exhaustive polish.
- You are not alone in the codebase. Do not revert unrelated edits. Adjust your implementation to accommodate existing code.
- Run focused validation for your slice if practical.

Return:
- concise summary
- exact files changed
- validation run
- notable deviations from ideal spec behavior if any
```

## Review Checklist For Each Returning Worker

- Does the diff actually implement the requested slice?
- Did it introduce unrelated changes?
- Does it preserve existing behavior unless intentionally changed?
- Do tests cover the new behavior end to end where appropriate?
- Did `yarn lint`, `yarn typecheck`, and `yarn test` pass locally after the worker returned?
- If not, send a correction worker before committing.

## Stop Conditions

Stop and ask the user if:

- the next step requires a design decision not settled by `docs/git-mesh.md`
- the worktree contains conflicting unexpected edits
- a required validation command fails for unclear environmental reasons
- the remaining gap is too large to bound cleanly in one slice

Otherwise, continue iterating.
