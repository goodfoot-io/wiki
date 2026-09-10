# Delegating stage 3

You are the owner. You have already run stages 1 and 2, so you hold the
components. Delegate stage 3 only; stages 1, 2, and 4 stay yours. You are the
only one who commits.

## Size the team

Count the flagged links left after stage 1:

| Flagged links | Workers to spawn |
|---|---|
| 9–20 | 2 |
| 21+ | 3 |

Split the components between the workers by flagged-link count, keeping one
for yourself to work while they run.

## Spawn

Spawn each worker by delegating to a subagent with `invoke_subagent`, named
`reconcile-worker-<i>`. Delegated subagents are not documented to inherit your
conversation, so the delegation message must carry the whole unit:

```text
Read [absolute path to ./procedure.md] and run its stage 3 on the unit below.
Only stage 3 — do not run stages 1, 2, or 4, and never commit.

Unit: <pages; every flagged link as target path, range, kind; shared target
files; range-overlap flags>

Report per page: classification per link, action taken, the decisive nonlocal
fact, the link diff (fragments and prose changed), whether links-reviewed was
bumped, and elapsed time. Flag any page you stopped on and why.
```

Check state with `manage_subagents` while workers run, and re-engage a live
worker with `send_message` when its unit needs a correction.

Units must be disjoint and must never separate two pages of the same
component.

## Collect

A worker that stops on unclear authority is reporting correctly — decide it
yourself per `./procedure.md`'s authority rules, or surface it in your own
report. Never bump `links-reviewed:` over a live disagreement.

When every report is in, run stage 4.
