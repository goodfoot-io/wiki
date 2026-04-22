# git mesh stale --verbose

Reference for the `--verbose` output of `git mesh stale`. Describes the
human-readable format, the equivalent `--format=json` shape, and the
purpose of every field.

`--verbose` is purely observational: it shows state, never recommendations.
Every field is a sha, a count, a timestamp, a path, a range, verbatim text,
or bytes. Interpretation — whether to re-anchor, patch a partner side,
remove the mesh, or escalate — is the reviewer's (or caller's) job.

All data is derived from the local object database. No network.

---

## 1. Human-readable output

One worked run covering every status code. The scenarios are drawn from
realistic cross-surface relationships the mesh is designed to track
(cross-language API calls, SQL column references, feature-flag strings,
paired numeric invariants, and so on).

### 1.1 MODIFIED (drifted contract surface)

```
$ git mesh stale --verbose frontend-backend-sync

mesh frontend-backend-sync
commit e0f92a3b8c1d0f5e6a7b2c4d9e1f0a3b8c1d0f5e
Author: John Wehr <john@example.com>
Date:   Wed Apr 15 11:14:03 2026 +0000
Anchor: a1b2c3d4  Wed Mar 23 10:02:17 2026 +0000  (23d ago, 47 commits back)
Refs:   main, origin/main

    ABC-123: front-end components reflect these back-end endpoints

    invariant: fetch call body shape matches handler request type

    Owner: team-billing

2 of 3 links stale:

link 63d239a4  MODIFIED

    F  src/Button.tsx       42,50              unchanged since anchor
    M  server/routes.ts     13,34 -> 15,36     3/22 lines, 14%

  Commits on server/routes.ts since anchor (4, by 2 authors):
    1ab2c3d4  Priya  3h    +1/-0    1/1 alive  fix: null check on session
    4e8b2c1a  Priya  2d    +3/-3    3/3 alive  refactor: extract session helper
    2d4e1b77  Priya  4d    rename              rename session helper
    9f1c0ab2  John   9d    +2/-0    2/2 alive  add request-id middleware

  Commits on src/Button.tsx since anchor: none.

  1ab2c3d4  Priya, 3h ago:

      fix: null check on session

      Follow-up to 4e8b2c1a; the extracted helper could return undefined
      when the session cookie was missing.

      Ticket: ABC-4822

  4e8b2c1a  Priya, 2d ago:

      refactor: extract session helper

      Pulling session lookup out of the charge handler so the new billing
      routes can reuse it. No behavior change intended; request/response
      shape unchanged.

      Ticket: ABC-4821
      Refs: 9f1c0ab2
      Reviewed-by: John Wehr <john@example.com>

  9f1c0ab2  John, 9d ago:

      add request-id middleware

      For end-to-end tracing across the new billing flow.

      Ticket: OBS-204

  Also in:
    billing-docs-sync  link a7f3e2c9  MODIFIED  overlaps B at 15,36

  diff --git a/server/routes.ts b/server/routes.ts
  index bb4c7e91..5f3a2b08 100644
  --- a/server/routes.ts     (a1b2c3d4)
  +++ b/server/routes.ts     (HEAD)
  @@ -13,22 +15,22 @@
  -router.post('/charge', (req, res) => {
  -  const { userId, amount } = req.body;
  -  const session = lookupSession(req.cookies.sid);
  +router.post('/charge', async (req, res) => {
  +  const { userId, amount, idempotencyKey } = req.body;
  +  const session = await sessionHelper.lookup(req);
     if (!session) return res.status(401).end();
     ...
  -  res.json(charge);
  +  res.json({ id: charge.id, status: charge.status });
   });

  src/Button.tsx @ 42,50 (unchanged since anchor):
      42  const res = await fetch('/api/charge', {
      43    method: 'POST',
      44    body: JSON.stringify({
      45      userId: user.id,
      46      amount: cents,
      47      idempotencyKey: key,
      48    }),
      49  });
      50  if (!res.ok) throw new ChargeError(await res.text());
```

### 1.2 MOVED (bytes identical, location shifted)

```
link a1b2c3d4  MOVED

    F  src/types.ts         8,15               unchanged since anchor
    V  server/schema.ts     20,27 -> 24,31     bytes identical

  Commits on server/schema.ts since anchor (1):
    5b6c7d8e  Chen   12d   +4/-0    4/4 alive  chore: add license header

  Commits on src/types.ts since anchor: none.

  5b6c7d8e  Chen, 12d ago:

      chore: add license header

      Adding the Apache 2.0 header to every file in server/ per legal
      request.

      Ticket: LEGAL-88
```

### 1.3 FRESH (both sides unchanged)

```
link 9e8f7d6c  FRESH

    F  src/Form.tsx         12,40
    F  server/validate.ts   5,22
```

### 1.4 MISSING (anchored bytes gone from one side)

```
link e4f5a6b7  MISSING

    F  src/flags.ts              12,12          literal "new-billing-flow" present
    X  config/features.yaml      42,48

  Removed by 8c9d0e1f  Chen, 5d ago:

      sunset: retire new-billing-flow

      Flag reached 100% rollout three weeks ago and has been stable.
      Removing from registry; call sites should become unconditional.

      Ticket: ABC-4919
      Closes: ABC-4128

  Other references to "new-billing-flow" at HEAD (pickaxe):
    src/flags.ts                 12
    src/billing/Checkout.tsx     19
    src/billing/Invoice.tsx      8

  config/features.yaml @ 42,48 (anchored bytes, preserved on link blob):
      42    new-billing-flow:
      43      default: false
      44      rollout:
      45        percent: 100
      46        cohorts:
      47          - beta
      48          - paid

  src/flags.ts @ 12,12 (unchanged since anchor):
      12  if (flags.enabled('new-billing-flow')) {
```

### 1.5 ORPHANED (anchor commit unreachable)

```
link f1e2d3c4  ORPHANED

    O  packages/auth/token.ts     88,104
    F  packages/auth/crypto.ts    12,40

  Anchor a1b2c3d4: unreachable.
    branches containing:  (none)
    reflog:               feat/token-v2, 12d ago
    fsck:                 unreachable, present in object database

  Link blob bb4c7e91 (refs/links/v1/f1e2d3c4): reachable.

  packages/auth/token.ts @ 88,104 (anchored bytes, preserved on link blob):
      88   function verifyToken(raw: string) {
      89     const [header, payload, sig] = raw.split('.');
      90     ...
     104   }

  Head match (content search for anchored bytes):
    packages/auth/token.ts  91,107  identical, +3-line offset

  packages/auth/crypto.ts @ 12,40 (unchanged since anchor):
      12  export async function verifySignature(...) {
      ...
      40  }
```

### 1.6 Status letter key

One-letter column prefix on each side line:

| Letter | Status | Meaning |
|---|---|---|
| `F` | FRESH | Current bytes equal anchored bytes. |
| `M` | MODIFIED | Some lines in the range rewritten; majority survive. |
| `R` | REWRITTEN | Majority of the range rewritten. |
| `V` | MOVED | Bytes identical; path or line numbers changed. |
| `X` | MISSING | No surviving location under the configured copy detection. |
| `O` | ORPHANED | Anchor commit no longer reachable from any ref. |

---

## 2. JSON output (`--format=json --verbose`)

One-to-one with the human output. Consumers that need a structured feed
read this; humans reading terminal output read §1.

```jsonc
{
  "version": 2,
  "mesh": "frontend-backend-sync",
  "commit": "e0f92a3b8c1d0f5e6a7b2c4d9e1f0a3b8c1d0f5e",
  "author": { "name": "John Wehr", "email": "john@example.com" },
  "date":   "2026-04-15T11:14:03Z",
  "message": "ABC-123: front-end components reflect these back-end endpoints\n\ninvariant: fetch call body shape matches handler request type\n\nOwner: team-billing\n",
  "anchor": {
    "sha":  "a1b2c3d4",
    "date": "2026-03-23T10:02:17Z",
    "refs": ["refs/heads/main", "refs/remotes/origin/main"],
    "distance_from_head": { "commits": 47, "days": 23 }
  },
  "summary": { "total": 3, "stale": 2, "fresh": 1 },
  "links": [

    {
      "id":     "63d239a4",
      "status": "MODIFIED",
      "sides": [
        {
          "label":   "A",
          "path":    "src/Button.tsx",
          "anchored": { "start": 42, "end": 50, "blob": "a9f1..." },
          "current":  { "start": 42, "end": 50, "blob": "a9f1..." },
          "status":  "FRESH"
        },
        {
          "label":   "B",
          "path":    "server/routes.ts",
          "anchored": { "start": 13, "end": 34, "blob": "bb4c7e91" },
          "current":  { "start": 15, "end": 36, "blob": "5f3a2b08" },
          "status":  "MODIFIED",
          "rewrite_ratio": 0.136
        }
      ],
      "commits_per_side": {
        "A": { "count": 0, "authors": 0, "entries": [] },
        "B": {
          "count":   4,
          "authors": 2,
          "entries": [
            {
              "sha":     "1ab2c3d4",
              "subject": "fix: null check on session",
              "body":    "Follow-up to 4e8b2c1a; the extracted helper could return undefined\nwhen the session cookie was missing.\n",
              "trailers": { "Ticket": ["ABC-4822"] },
              "notes":   null,
              "author":  "Priya",
              "at":      "2026-04-22T08:14:00Z",
              "stat":    { "add": 1, "del": 0, "rename_only": false },
              "survives": { "alive": 1, "contributed": 1 }
            },
            {
              "sha":     "4e8b2c1a",
              "subject": "refactor: extract session helper",
              "body":    "Pulling session lookup out of the charge handler so the new billing\nroutes can reuse it. No behavior change intended; request/response\nshape unchanged.\n",
              "trailers": {
                "Ticket":      ["ABC-4821"],
                "Refs":        ["9f1c0ab2"],
                "Reviewed-by": ["John Wehr <john@example.com>"]
              },
              "notes":   null,
              "author":  "Priya",
              "at":      "2026-04-20T09:12:00Z",
              "stat":    { "add": 3, "del": 3, "rename_only": false },
              "survives": { "alive": 3, "contributed": 3 }
            },
            {
              "sha":     "2d4e1b77",
              "subject": "rename session helper",
              "body":    "",
              "trailers": {},
              "notes":   null,
              "author":  "Priya",
              "at":      "2026-04-18T15:30:00Z",
              "stat":    { "add": 0, "del": 0, "rename_only": true },
              "survives": { "alive": 0, "contributed": 0 }
            },
            {
              "sha":     "9f1c0ab2",
              "subject": "add request-id middleware",
              "body":    "For end-to-end tracing across the new billing flow.\n",
              "trailers": { "Ticket": ["OBS-204"] },
              "notes":   null,
              "author":  "John",
              "at":      "2026-04-13T11:00:00Z",
              "stat":    { "add": 2, "del": 0, "rename_only": false },
              "survives": { "alive": 2, "contributed": 2 }
            }
          ]
        }
      },
      "commits_touching_both": [],
      "cross_mesh_overlap": [
        {
          "mesh":    "billing-docs-sync",
          "link_id": "a7f3e2c9",
          "status":  "MODIFIED",
          "side":    "B",
          "overlap": { "start": 15, "end": 36 }
        }
      ],
      "drifted_diff": {
        "side":         "B",
        "path":         "server/routes.ts",
        "anchored_oid": "bb4c7e91",
        "current_oid":  "5f3a2b08",
        "hunk_header":  "@@ -13,22 +15,22 @@",
        "unified":      "@@ -13,22 +15,22 @@\n-router.post('/charge', (req, res) => {\n-  const { userId, amount } = req.body;\n..."
      },
      "fresh_content": [
        {
          "side":    "A",
          "path":    "src/Button.tsx",
          "start":   42,
          "end":     50,
          "content": "const res = await fetch('/api/charge', {\n  method: 'POST',\n  body: JSON.stringify({\n    userId: user.id,\n    amount: cents,\n    idempotencyKey: key,\n  }),\n});\nif (!res.ok) throw new ChargeError(await res.text());\n"
        }
      ]
    },

    {
      "id":     "a1b2c3d4",
      "status": "MOVED",
      "sides": [
        {
          "label": "A",
          "path":  "src/types.ts",
          "anchored": { "start": 8, "end": 15, "blob": "c0a1..." },
          "current":  { "start": 8, "end": 15, "blob": "c0a1..." },
          "status": "FRESH"
        },
        {
          "label": "B",
          "path":  "server/schema.ts",
          "anchored": { "start": 20, "end": 27, "blob": "d4b2..." },
          "current":  { "start": 24, "end": 31, "blob": "d4b2..." },
          "status": "MOVED"
        }
      ],
      "commits_per_side": {
        "A": { "count": 0, "authors": 0, "entries": [] },
        "B": {
          "count":   1,
          "authors": 1,
          "entries": [
            {
              "sha":     "5b6c7d8e",
              "subject": "chore: add license header",
              "body":    "Adding the Apache 2.0 header to every file in server/ per legal\nrequest.\n",
              "trailers": { "Ticket": ["LEGAL-88"] },
              "notes":   null,
              "author":  "Chen",
              "at":      "2026-04-10T14:02:00Z",
              "stat":    { "add": 4, "del": 0, "rename_only": false },
              "survives": { "alive": 4, "contributed": 4 }
            }
          ]
        }
      },
      "commits_touching_both": [],
      "cross_mesh_overlap": []
    },

    {
      "id":     "9e8f7d6c",
      "status": "FRESH",
      "sides": [
        {
          "label": "A",
          "path":  "src/Form.tsx",
          "anchored": { "start": 12, "end": 40, "blob": "e5c3..." },
          "current":  { "start": 12, "end": 40, "blob": "e5c3..." },
          "status": "FRESH"
        },
        {
          "label": "B",
          "path":  "server/validate.ts",
          "anchored": { "start": 5, "end": 22, "blob": "f6d4..." },
          "current":  { "start": 5, "end": 22, "blob": "f6d4..." },
          "status": "FRESH"
        }
      ]
    }

  ]
}
```

### 2.1 MISSING link shape

Distinct from MODIFIED: no `commits_per_side` trajectory (the content
vanished rather than evolving), no `drifted_diff` (nothing to diff
against). Adds `removal`, `pickaxe_hits`, and `anchored_content`:

```jsonc
{
  "id":     "e4f5a6b7",
  "status": "MISSING",
  "sides": [
    {
      "label": "A",
      "path":  "src/flags.ts",
      "anchored": { "start": 12, "end": 12, "blob": "..." },
      "current":  { "start": 12, "end": 12, "blob": "..." },
      "status": "FRESH",
      "note":   "literal \"new-billing-flow\" present"
    },
    {
      "label": "B",
      "path":  "config/features.yaml",
      "anchored": { "start": 42, "end": 48, "blob": "..." },
      "current":  null,
      "status": "MISSING"
    }
  ],
  "removal": {
    "sha":     "8c9d0e1f",
    "subject": "sunset: retire new-billing-flow",
    "body":    "Flag reached 100% rollout three weeks ago and has been stable.\nRemoving from registry; call sites should become unconditional.\n",
    "trailers": { "Ticket": ["ABC-4919"], "Closes": ["ABC-4128"] },
    "notes":   null,
    "author":  "Chen",
    "at":      "2026-04-17T11:00:00Z"
  },
  "pickaxe_hits": [
    { "path": "src/flags.ts",             "line": 12 },
    { "path": "src/billing/Checkout.tsx", "line": 19 },
    { "path": "src/billing/Invoice.tsx",  "line": 8  }
  ],
  "pickaxe_query": "new-billing-flow",
  "anchored_content": {
    "side":          "B",
    "path":          "config/features.yaml",
    "start":         42,
    "end":           48,
    "content":       "new-billing-flow:\n  default: false\n  rollout:\n    percent: 100\n    cohorts:\n      - beta\n      - paid\n",
    "preserved_via": "link_blob"
  },
  "fresh_content": [
    {
      "side":    "A",
      "path":    "src/flags.ts",
      "start":   12,
      "end":     12,
      "content": "if (flags.enabled('new-billing-flow')) {\n"
    }
  ]
}
```

### 2.2 ORPHANED link shape

Adds `anchor_recovery`, `link_blob`, `anchored_content`, and `head_match`:

```jsonc
{
  "id":     "f1e2d3c4",
  "status": "ORPHANED",
  "sides": [
    {
      "label":    "A",
      "path":     "packages/auth/token.ts",
      "anchored": { "start": 88, "end": 104, "blob": "a7b8..." },
      "current":  null,
      "status":   "ORPHANED"
    },
    {
      "label":    "B",
      "path":     "packages/auth/crypto.ts",
      "anchored": { "start": 12, "end": 40, "blob": "c9d0..." },
      "current":  { "start": 12, "end": 40, "blob": "c9d0..." },
      "status":   "FRESH"
    }
  ],
  "anchor_recovery": {
    "sha":                 "a1b2c3d4",
    "branches_containing": [],
    "reflog": [
      { "branch": "feat/token-v2", "at": "2026-04-10T09:00:00Z" }
    ],
    "fsck": { "status": "unreachable", "in_odb": true }
  },
  "link_blob": {
    "oid":       "bb4c7e91",
    "ref":       "refs/links/v1/f1e2d3c4",
    "reachable": true
  },
  "anchored_content": {
    "side":          "A",
    "path":          "packages/auth/token.ts",
    "start":         88,
    "end":           104,
    "content":       "function verifyToken(raw: string) {\n  const [header, payload, sig] = raw.split('.');\n...\n}\n",
    "preserved_via": "link_blob"
  },
  "head_match": {
    "side":       "A",
    "path":       "packages/auth/token.ts",
    "start":      91,
    "end":        107,
    "similarity": "identical",
    "offset":     3
  },
  "fresh_content": [
    {
      "side":    "B",
      "path":    "packages/auth/crypto.ts",
      "start":   12,
      "end":     40,
      "content": "export async function verifySignature(...) {\n...\n}\n"
    }
  ]
}
```

---

## 3. Field reference

Every field, what it contains, how it's computed, and how a reviewer (or
a consuming tool) uses it.

### 3.1 Mesh header

#### `mesh <name>`

**JSON**: `mesh` (string).
**Content**: The mesh's name. Identical to the ref suffix
`refs/meshes/v1/<name>`.
**Use**: Self-identifying output; a reviewer reading a pasted block can
tell which mesh produced it. Consumers route reports to owners by name.

#### `commit <full-sha>`

**JSON**: `commit` (string).
**Content**: Full 40-char sha of the mesh's tip commit — the current state
of the mesh's `links` file and message.
**Computed from**: `git rev-parse refs/meshes/v1/<name>`.
**Use**: Pinning the observed state. Re-runs against the same `commit`
reproduce the same report (modulo HEAD changes on the tracked files).

#### `Author:` / `Date:`

**JSON**: `author: { name, email }`, `date` (ISO 8601).
**Content**: Author and commit date of the mesh's tip commit. Mirrors
`git show` header fields.
**Use**: Who last edited the mesh, and when. A stale mesh that nobody has
touched in six months is a different signal from one edited yesterday.

#### `Anchor: <sha>  <date>  (<rel-age>, <N> commits back)`

**JSON**: `anchor: { sha, date, distance_from_head: { commits, days } }`.
**Content**: The commit every Link in the mesh is anchored to, the
absolute date of that commit, and the distance from HEAD in commits and
days.
**Computed from**: The `anchor_sha` field in each Link record (they all
share an anchor within a mesh by convention), plus
`git rev-list --count <anchor>..HEAD` and the commit's authored date.
**Use**: Bounds the drift window — how much history the resolver has to
walk. Also surfaces stale anchors: a year-old anchor with a live HEAD is
a mesh that's been neglected even when every Link reads `FRESH`.

#### `Refs: <ref-name>, <ref-name>, ...`

**JSON**: `anchor.refs` (array of ref paths).
**Content**: Refs that reach the anchor commit.
**Computed from**: `git branch --contains <anchor>` plus
`git for-each-ref --contains <anchor> refs/remotes/`.
**Use**: Confirms the anchor is on a durable branch (e.g. `main`,
`origin/main`). An anchor reachable only from the reflog is a latent
ORPHANED candidate — it will break the moment `git gc` runs.

#### Indented message block

**JSON**: `message` (string, full commit message).
**Content**: The commit message of the mesh's tip, rendered with git's
four-space indent. Subject, blank line, body, trailers — all as written.
**Use**: Declares the relationship's purpose and any invariant the
reviewer is supposed to check. With the prescriptive fields gone,
this message is the primary guide to what "stale" means for this mesh.

#### `N of M links stale:`

**JSON**: `summary: { total, stale, fresh }`.
**Content**: Count of total Links in the mesh, count whose status is not
`FRESH`, and count whose status is `FRESH`.
**Use**: Header-then-body cadence borrowed from `git status`. CI jobs
that want a one-line summary read this; interactive users skim it before
diving into individual Link blocks.

### 3.2 Per-link header

#### `link <id>  <STATUS>`

**JSON**: `id`, `status` at the link level.
**Content**: The Link's id (UUID, first 8 chars by default; full 40
shown with `--no-abbrev`) and the worse of its two side statuses.
**Use**: Self-identifying per-Link block boundary. The status is the
top-level signal that determines which subsequent fields appear.

### 3.3 Per-side status line

```
F  src/Button.tsx       42,50              unchanged since anchor
M  server/routes.ts     13,34 -> 15,36     3/22 lines, 14%
```

**JSON**: Each element of `sides[]`:
```jsonc
{
  "label": "A" | "B",
  "path":  "...",
  "anchored": { "start", "end", "blob" },
  "current":  { "start", "end", "blob" } | null,
  "status":  "...",
  "rewrite_ratio": 0.136     // optional, MODIFIED/REWRITTEN only
}
```

**Content**: Status letter (see §1.6), path, anchored range, optional
current range (`-> <start>,<end>` when location changed), and a delta
summary (`N/M lines, P%` for partial rewrites, `bytes identical` for
MOVED, `unchanged since anchor` for FRESH, nothing for MISSING/ORPHANED).

**Computed from**: The per-side resolver pass (`git log -L` plus byte
compare, per §5 of the main git-mesh spec).

**Use**: The densest "what happened to this side" line in the output.
Reviewers scan these first. The status letter and the range delta
together tell them whether they need to read further.

### 3.4 Commits on each side (one-liners)

```
Commits on server/routes.ts since anchor (4, by 2 authors):
  1ab2c3d4  Priya  3h    +1/-0    1/1 alive  fix: null check on session
  ...
Commits on src/Button.tsx since anchor: none.
```

**JSON**: `commits_per_side.<label>.entries[]` with `count` and `authors`
at the object level.

**Per-entry fields**:

| Column | JSON path | Meaning |
|---|---|---|
| `<sha>` | `sha` | Short sha of the commit. |
| `<author>` | `author` | Author name. |
| `<age>` | (computed from `at`) | `git log --date=relative`-style. |
| `+N/-M` or `rename` | `stat: { add, del, rename_only }` | Lines added/deleted inside the range at this commit; `rename` if the commit only changed the file's path. |
| `X/Y alive` | `survives: { alive, contributed }` | Lines this commit contributed that are still blamed to it at HEAD, over total lines it contributed. Absent for rename-only commits. |
| `<subject>` | `subject` | Commit subject, verbatim. |

**Computed from**:
- One-liner list: `git log --raw --numstat --format='%H%x00%an%x00%at%x00%s%x00' <anchor>..HEAD -- <path>`.
- Survives count: `git blame --porcelain -w -C -L <current-start>,<current-end> HEAD -- <path>`, then count lines whose blame sha equals each trajectory sha.

**Use**:
- The one-liner list answers "who touched this side since the Link was
  anchored?"
- The author count (`by 2 authors`) signals concentrated vs. distributed
  churn — a single-author run is usually a refactor; many authors often
  means an ongoing feature.
- The `alive` ratio signals whether a commit's contribution is still
  load-bearing. Commits with `0/N alive` were entirely overwritten by
  later commits and are usually not worth reading.

#### `Commits on <partner-path> since anchor: none.` and `Commits touching both sides: <N>`

**JSON**: `commits_per_side.<label>.count` (zero implies the human line)
and `commits_touching_both` (array of sha strings, empty if none).

**Content**:
- If the partner side has no commits in the window, the output states it
  explicitly.
- If both sides have commits, a third line lists the intersection —
  commits whose changes landed on both paths in a single commit.

**Computed from**: Intersection of the two per-side commit lists.

**Use**: The single most important signal for whether drift was
coordinated. Zero both-touching commits plus one-sided commit activity
means the partner side was never updated alongside the changes; all
commits being both-touching means everything was coordinated.

### 3.5 Commit body expansion

```
4e8b2c1a  Priya, 2d ago:

    refactor: extract session helper

    Pulling session lookup out of the charge handler so the new billing
    routes can reuse it. No behavior change intended; request/response
    shape unchanged.

    Ticket: ABC-4821
    Refs: 9f1c0ab2
    Reviewed-by: John Wehr <john@example.com>
```

**JSON**: Each `commits_per_side.<label>.entries[i]` has:
- `subject`, `body`, `trailers` (map of trailer key to list of values),
  `notes` (nullable string).

**Shown for which commits**: Every commit in the trajectory with
`survives.contributed > 0` (i.e. the commit contributed lines and at
least some may remain). Rename-only and fully-overwritten commits are
shown in the one-liner list only.

**Computed from**:
- Body: `git log --format='%B' -1 <sha>`.
- Trailers: `git interpret-trailers --parse` piped over the body.
- Notes: `git notes show <sha>` when `refs/notes/commits` exists.

**Use**:
- The verbatim body is the richest "why" signal in the object database.
  With no forge lookup in the picture, this is where declared intent
  lives.
- Trailers surface ticket references, reviewers, and cross-commit refs
  ("Refs: 9f1c0ab2") that help the reviewer pull a broader narrative
  together.
- Notes carry post-hoc annotations when a team adds context without
  rewriting history.

### 3.6 Also in (cross-mesh overlap)

```
Also in:
  billing-docs-sync  link a7f3e2c9  MODIFIED  overlaps B at 15,36
```

**JSON**: `cross_mesh_overlap[]` with `mesh`, `link_id`, `status`,
`side`, and `overlap: { start, end }`.

**Content**: Other meshes that reference any range overlapping this
Link's current ranges.

**Computed from**: `git for-each-ref refs/meshes/v1/*` plus a read of
each `links` blob and each Link record, cross-referenced against the
current ranges.

**Use**: Prevents "fix one mesh, leave siblings stale" bugs. A route
that's tracked by both a contract mesh and a docs-sync mesh should be
reconciled in both; this section makes the coupling visible at triage
time.

### 3.7 Drifted-side diff block

```
diff --git a/server/routes.ts b/server/routes.ts
index bb4c7e91..5f3a2b08 100644
--- a/server/routes.ts     (a1b2c3d4)
+++ b/server/routes.ts     (HEAD)
@@ -13,22 +15,22 @@
 ...
```

**JSON**: `drifted_diff: { side, path, anchored_oid, current_oid, hunk_header, unified }`.

**Content**: A unified diff of the drifted side's anchored bytes against
its current bytes, in `git diff` format with real blob OIDs in the
`index` line.

**Computed from**: `git diff --no-color <anchored-blob> <current-blob>`
plus a post-processing step to insert the path and commit decorations
on the `---` / `+++` lines.

**Use**: The ground truth for "what changed." The reviewer reads this
after the commit body summaries if the body wasn't explicit, and before
any edit they plan to make on the partner side.

**When shown**: Only for Links where at least one side is MODIFIED or
REWRITTEN. Omitted for MOVED (bytes identical), FRESH, MISSING (content
gone, no counterpart to diff against), and ORPHANED (shown via
`anchored_content` + `head_match` instead).

### 3.8 Fresh-side content block

```
src/Button.tsx @ 42,50 (unchanged since anchor):
    42  const res = await fetch('/api/charge', {
    ...
```

**JSON**: `fresh_content[]` with `side`, `path`, `start`, `end`, `content`.

**Content**: The current bytes of any side still in `FRESH` status,
annotated with line numbers. Shown so the reviewer can eyeball the
partner side alongside the drifted side's diff.

**Computed from**: `git cat-file -p HEAD:<path>` sliced to
`<start>,<end>`.

**Use**: For contract-shaped meshes, this is the side the drifted side
should still match. For invariant-shaped meshes (paired timeouts,
sanitizer ordering), this is the fixed reference the reviewer checks
the other side against.

**When shown**: Always shown for FRESH sides of a MODIFIED/MOVED/
MISSING/ORPHANED Link. For a fully-FRESH Link, only the side lines are
printed — no content blocks — because there's nothing to compare.

### 3.9 MISSING: removal commit

```
Removed by 8c9d0e1f  Chen, 5d ago:

    sunset: retire new-billing-flow
    ...
```

**JSON**: `removal: { sha, subject, body, trailers, notes, author, at }`.

**Content**: The single commit whose diff caused the anchored bytes to
disappear from the tracked path, with full body and trailers.

**Computed from**: Pickaxe —
`git log --all -S '<distinctive-anchored-substring>' -- <path>` — taking
the newest result. Within the run, cached by `(bytes-hash, path)` so
repeated pickaxes don't re-pay.

**Use**: For `MISSING`, the removal commit's body is the primary
interpretation signal. "sunset: retire" plus a body saying "call sites
should become unconditional" tells the reviewer to remove the mesh and
patch the partners. "fix: accidental remove, will restore" tells them to
revert and wait. Same data, opposite actions, both decided by the
reviewer from the body.

### 3.10 MISSING: pickaxe hits

```
Other references to "new-billing-flow" at HEAD (pickaxe):
  src/flags.ts                 12
  src/billing/Checkout.tsx     19
  src/billing/Invoice.tsx      8
```

**JSON**: `pickaxe_hits[]` and `pickaxe_query`.

**Content**: A list of `(path, line)` pairs in the current tree where
the distinctive anchored substring still appears, plus the exact query
used.

**Computed from**: `git grep -nF '<substring>' HEAD -- ':!/<already-tracked-path>'`.

**Use**: Fanout. A feature-flag key that was removed from the registry
but still appears in five call sites is the "things you still need to
fix" list. For `MISSING` on a config key, config name, error code, or
any other distinctive string, this list is often the unit of reconcile
work.

### 3.11 MISSING: anchored content

```
config/features.yaml @ 42,48 (anchored bytes, preserved on link blob):
    42    new-billing-flow:
    ...
```

**JSON**: `anchored_content: { side, path, start, end, content, preserved_via }`.

**Content**: The anchored bytes of the missing side, recovered from the
Link's stored blob rather than from the (now-deleted) location in the
tree.

**Computed from**: `git cat-file -p <link-side.blob>` sliced to
`<anchored-start>,<anchored-end>`.

**Use**: Shows the reviewer what was there. Confirms the removal matched
expectations (the reviewer reads the bytes and says "yes, that's the
flag definition we were tracking"). Also useful when the reviewer needs
to restore the bytes — the original content is there for the copying.

### 3.12 ORPHANED: anchor recovery

```
Anchor a1b2c3d4: unreachable.
  branches containing:  (none)
  reflog:               feat/token-v2, 12d ago
  fsck:                 unreachable, present in object database
```

**JSON**: `anchor_recovery: { sha, branches_containing, reflog, fsck }`.

**Content**: Reachability breakdown for the anchor commit — which refs
(if any) contain it, whether it appears in the reflog with a branch
name, and whether `fsck` still sees it in the object database.

**Computed from**:
- `branches_containing`: `git branch -a --contains <anchor>` plus
  `git for-each-ref --contains <anchor>`.
- `reflog`: scan of `.git/logs/refs/**` matching the anchor sha.
- `fsck`: single `git fsck --unreachable` at startup, cached for the run.

**Use**: Diagnoses what happened. `reflog` entry without a
`branches_containing` entry means the branch was rebased or force-pushed.
Empty everywhere means `git gc` has already run; the bytes are only
recoverable via the Link blob (next field).

### 3.13 ORPHANED: link blob

```
Link blob bb4c7e91 (refs/links/v1/f1e2d3c4): reachable.
```

**JSON**: `link_blob: { oid, ref, reachable }`.

**Content**: OID of the Link's stored blob and whether it's still
reachable through its Link ref.

**Computed from**: `git rev-parse refs/links/v1/<link-id>`.

**Use**: Confirms the anchored bytes can be recovered even when the
anchor commit cannot. As long as the Link ref exists, the Link blob is
reachable by definition; this field exists to make that guarantee
visible (and to surface the rare case of a corrupted Link ref).

### 3.14 ORPHANED: head match

```
Head match (content search for anchored bytes):
  packages/auth/token.ts  91,107  identical, +3-line offset
```

**JSON**: `head_match: { side, path, start, end, similarity, offset } | null`.

**Content**: Best-effort location at HEAD where the anchored bytes appear,
with `similarity` (`identical` / `partial`) and `offset` (line-number
delta from the anchored location).

**Computed from**: A content search over HEAD's tree — `git grep -F`
on a distinctive slice of the anchored bytes, with a byte-equality check
on the matching range.

**Use**: Tells the reviewer whether the anchored content still exists
somewhere, even though its original history is broken. `identical` +
small offset is the common case after a rebase; the reviewer can
recreate the Link at the new location with confidence. Missing
(`head_match: null`) means the anchored content may also be gone, and
the reviewer is looking at two concurrent problems.

---

## 4. Notes on computation

### 4.1 Which git objects are read

Per Link:

- `git rev-parse refs/meshes/v1/<name>` — mesh tip.
- `git cat-file -p <commit>` — mesh tip commit.
- `git cat-file -p <commit>^{tree}:links` — mesh's `links` file.
- `git rev-parse refs/links/v1/<link-id>` + `git cat-file -p <blob>` —
  each Link record.
- `git log -L <range>:<path> <anchor>..HEAD` per side — resolver's range
  walker.
- `git cat-file -p HEAD:<current-path>` per side — current bytes.
- `git log --raw --numstat --format=... <anchor>..HEAD -- <path>` per
  side — trajectory.
- `git blame --porcelain -w -C -L <current-range> HEAD -- <path>` per
  drifted side — survival counts.
- `git interpret-trailers --parse` piped over bodies — trailers.
- `git notes show <sha>` per trajectory commit — only if
  `refs/notes/commits` exists.
- `git diff <anchored-blob> <current-blob>` per drifted side — diff
  block.
- `git for-each-ref refs/meshes/v1/*` once plus one blob read per mesh
  — cross-mesh overlap.
- `git log --all -S '<bytes>' -- <path>` per MISSING side — pickaxe.
- `git grep -nF` per MISSING side — pickaxe hits at HEAD.
- `git branch --contains`, `git for-each-ref --contains`, reflog scan,
  one `git fsck --unreachable` per run — ORPHANED reachability.
- `git grep -F` over HEAD plus byte compare — ORPHANED head match.

All local. No network, no forge calls, no auth.

### 4.2 Caching within a run

- `git fsck --unreachable` output — once per run, reused for every
  ORPHANED Link.
- Pickaxe results — keyed by `(bytes-hash, path)`, shared across Links
  that happen to anchor the same content in the same file.
- Trajectory results — keyed by `(path, anchor)`, shared across Links
  in the same mesh that name the same path (rare but possible).

### 4.3 What's deliberately not in the output

- Prescriptive fields: no `recommendation`, no `reconcile_command`, no
  confidence score, no classification of the culprit commit's intent.
- Semantic analysis: no shared-identifier check, no contract-surface
  diff. Tokenization is language-agnostic at best and language-wrong at
  worst; the diff block and the partner-side content block give the
  reviewer the same information without the tool's gloss.
- Any signal requiring network access: no forge links, no PR titles, no
  linked-issue expansion.

The test of the design: a reviewer (human or agent) given this output
and no other access to the repo should be able to decide, for each
stale Link, which of { re-anchor, re-anchor after patching the partner,
remove the mesh, recreate from blob, escalate } is the right move. If
a case consistently needs data not shown here, that's a gap in the
spec, not in the reviewer.
