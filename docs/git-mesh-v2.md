# Git Mesh

A system for attaching tracked, updatable metadata to portions of code
in a git repository. Data lives in git's object database under custom
refs — nothing is written to the working tree, and the system has no
sidecar database.

## 1. Overview

There are exactly two primitives:

- **Range** — an immutable anchor to an exact set of lines in a file at
  a specific commit. Ranges are content-addressed, shared, and cheap.
- **Mesh** — a mutable, commit-backed record that groups a set of Range
  references together under a free-text message. Meshes are how humans
  name and describe the relationships between anchored ranges, and they
  evolve over time as the code evolves.

Staleness — whether a Range's anchored bytes still exist somewhere in
the current tree — is **always computed**, never stored. The repo's
history is the only source of truth; the stored records are minimal.

## 2. Concepts

### 2.1 Range

A Range is an anchor to a single line range in a file, captured at a
specific commit. The Range carries one `anchor_sha`, one `path`, and
the `(start, end, blob)` of the anchored lines. Given a Range, the tool
can ask:

- *Where is this range now?* — `git log -L` walks forward from the
  anchor commit, following the range through diff hunks, renames, and
  copies.
- *Is the content still the same?* — extract the anchored bytes from the
  anchor blob; extract the bytes at the resolved location; compare.

Ranges are immutable: once written, the ref points at its blob forever
(or until deleted).

### 2.2 Mesh

A Mesh is a named set of Range references plus a free-text message. It
expresses "these anchored ranges belong together, and here is why." All
ranges in a mesh participate in a single named relationship — the mesh
name carries the semantic intent. There are no stored pairwise
associations between ranges; if two independent relationships need
tracking, that is two meshes.

A Mesh has exactly four pieces of state:

- **name** — the identity of the Mesh, carried by the ref name
  (`refs/meshes/v1/<name>`). Mirrors branches: the name *is* the mesh,
  and git's ref machinery handles collisions, renames, atomic updates,
  and sync.
- **ranges** — a sorted, deduplicated set of Range ids. Each id names a
  Range currently considered part of the relationship. Stored as one
  line per id in a single file inside the Mesh commit's tree.
- **config** — resolver options (`copy-detection`, `ignore-whitespace`)
  that apply to all ranges in the mesh. Stored as a `config` file in
  the Mesh commit's tree. Syncs with the mesh.
- **message** — a git-commit-message-style string describing the
  relationship. This *is* the commit's message; it is not duplicated
  anywhere in the tree.

A Mesh is mutable: edits write new commits on the Mesh's ref; the parent
chain records every past state. Per-edit history (which Range was added
or removed) is recovered by diffing a commit's tree against its parent;
it is not denormalized into the stored record.

### 2.3 Staleness

Staleness is a per-Range property, always computed on query:

- **Range status** — byte equality (modulo the mesh's `ignore_whitespace`
  setting) between the anchored bytes and the bytes at the resolved location.
- **Mesh** has no single aggregate status. `status`/`show` report each
  Range's status individually; callers that want a one-line summary
  decide their own aggregation rule (e.g. "any Range not `Fresh`" →
  needs attention).

The stored records never carry status fields. Every query recomputes
against HEAD, so the answer always matches the repo as it is now.

## 3. Storage model

### 3.1 Ref layout

```
refs/ranges/v1/<rangeId>   →   blob      →   Range record (text)
refs/meshes/v1/<name>      →   commit    →   tree  →  ranges  (text file)
                                   │
                                   └── parent: previous Mesh commit (or none)
```

- Range refs point directly at a content-addressed text blob. Identical
  Range payloads share a blob in the object database automatically.
- Mesh refs are named by the user. The commit's tree contains two files,
  `ranges` and `config`. The commit's **message** is the Mesh's message
  — no duplication in the tree. The commit's parent pointers form the
  Mesh's edit history.

### 3.2 Versioned namespace

The `v1` segment encodes the schema version of the stored records. A
reader can enumerate only shapes it understands (`git for-each-ref
refs/ranges/v1/`, likewise for meshes) without opening any blob.
Refspecs can filter by version. A future breaking change introduces
`refs/ranges/v2/*` and `refs/meshes/v2/*`; v1 records remain readable
under their own namespace indefinitely. There is no implicit migration.

### 3.3 Why these git objects

- **Range → blob (text).** Immutable, content-addressed, dedup-friendly.
  Writing is `git hash-object -w`; the id lives in the ref name. The
  on-disk format mirrors git's own commit/tag header style — `key SP
  value\n` lines, TAB-separated paths — so it's readable, compact, and
  parseable line-by-line.
- **Mesh → commit.** Mutable tip with a parent chain is exactly what
  git branches are; the same primitive gives edit history for free,
  captures author/date per edit, carries the Mesh's message as the
  commit message, and supports real three-way merges when two branches
  edit the same Mesh concurrently. The user-facing name is the ref name,
  just like a branch.

### 3.4 Name and id format

Both `<rangeId>` and `<name>` must be ref-legal path components: no
slashes, no whitespace, no control characters, and not a leading `-`.

- `<rangeId>` is always a UUID. Range refs are internal; users never see
  or type them.
- `<name>` is user-chosen on first write. It is the Mesh's only
  identity, and the same name cannot be used by two Meshes (ref-level
  collision, caught by `update-ref` CAS). Names follow the same rules
  git applies to branch names.

## 4. Data shapes

All types below describe the v1 on-disk shape. Every field is required;
defaults are applied at creation time so stored records fully
self-describe their resolver behaviour. JSON field names are camelCase
(via `serde(rename_all = "camelCase")`); Rust fields are snake_case.

### 4.1 Range

**On-disk format** (commit-object-style text, stored as the blob at
`refs/ranges/v1/<rangeId>`):

```
anchor <sha>
created <iso-8601>
range <start> <end> <blob>\t<path>
```

- Headers are `key SP value\n`. Unknown headers are tolerated (future
  additive extensions don't break v1 readers).
- The `range` line carries three space-separated fields, then a `\t`,
  then the path. Paths may contain spaces; no other field may, so the
  TAB unambiguously terminates the field block — the same dodge git uses
  in tree entries.
- Resolver options (`copy-detection`, `ignore-whitespace`) are
  mesh-level settings stored in the mesh commit's tree (see §4.3),
  not per-range fields.
- Trailing newline; no blank lines.

Typical size is ~80 bytes.

**Rust types** (ser/de is a hand-written parser, not serde-JSON):

```rust
/// In-memory representation of the Range record stored at
/// refs/ranges/v1/<rangeId>. The id itself is the ref name suffix and
/// is not repeated in the blob.
#[derive(Clone, Debug)]
pub struct Range {
    /// Commit this range was anchored to at creation.
    pub anchor_sha: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// File path at the anchor commit.
    pub path: String,
    /// 1-based, inclusive line range.
    pub start: u32,
    pub end: u32,
    /// Blob OID of `path` at `anchor_sha`. Freezes the exact anchored
    /// bytes and keeps the range verifiable even if `anchor_sha` becomes
    /// unreachable.
    pub blob: String,
}

/// -C levels for `git log -L` copy detection. Stored in mesh config,
/// not in the range record. Serialized as the kebab-case variant name:
/// `off`, `same-commit`, `any-file-in-commit`, `any-file-in-repo`.
///
/// * `Off`              — no -C
/// * `SameCommit`       — -C       (copies from files modified in the same commit)
/// * `AnyFileInCommit`  — -C -C    (copies from any file touched in the commit)
/// * `AnyFileInRepo`    — -C -C -C (copies from any file in the tree; expensive)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CopyDetection {
    Off,
    SameCommit,
    AnyFileInCommit,
    AnyFileInRepo,
}

pub struct MeshConfig {
    pub copy_detection: CopyDetection,
    pub ignore_whitespace: bool,
}

pub const DEFAULT_COPY_DETECTION: CopyDetection = CopyDetection::SameCommit;
pub const DEFAULT_IGNORE_WHITESPACE: bool = false;
```

### 4.2 Mesh

**On-disk shape.** A Mesh is a commit whose tree contains two files,
`ranges` and `config`, and whose commit message is the Mesh's message.
No `mesh.json`, no tree-level message duplication.

```
refs/meshes/v1/<name>
└── commit
    ├── message: "<subject>\n\n<body>"   ← the Mesh's message
    └── tree
        ├── ranges                       ← text file, one Range id per line
        └── config                       ← text file, key-value resolver options
```

The `ranges` file:

```
0a1b2c3d4e5f...
4d5e6f7a8b9c...
8e9f0a1b2c3d...
```

- One Range id per line.
- Sorted ascending; duplicates removed on write.
- Trailing newline; no blank lines.

The `config` file:

```
copy-detection same-commit
ignore-whitespace false
```

- One `key SP value` per line.
- Written on every mesh commit. Defaults are written explicitly so the
  stored record is fully self-describing.
- Syncs with the mesh — config history is visible in `git log -p`.

**Rust types** (assembled on read from the commit, `ranges`, and `config`
blobs; serialized back the same way on write):

```rust
#[derive(Clone, Debug)]
pub struct Mesh {
    /// The Mesh's name (ref suffix; the identity).
    pub name: String,
    /// Active Range ids. Canonical order: sorted ascending; deduped.
    pub ranges: Vec<String>,
    /// The commit's message.
    pub message: String,
    /// Resolver options for all ranges in this mesh.
    pub config: MeshConfig,
}
```

All addressing data lives in the Range blobs that `ranges` points at;
the Mesh commit itself stores the range pointers, the config, and the
message.

**Invariant:** within a single Mesh, no two Ranges may share the same
`(path, start, end)`. Writes that would violate this error out. This
lets every command address a Range by its location alone — no id suffix,
no disambiguation step.

### 4.3 Mesh config

Resolver options for a mesh are stored in the `config` file of the mesh
commit's tree. All ranges in the mesh inherit these settings. Config is
staged and committed alongside range changes — it is not local-only state.

| Key | Values | Default |
|---|---|---|
| `copy-detection` | `off`, `same-commit`, `any-file-in-commit`, `any-file-in-repo` | `same-commit` |
| `ignore-whitespace` | `true`, `false` | `false` |

### 4.4 Computed views

```rust
/// Declaration order is best → worst; `Ord` derives a total order so
/// callers that want a one-line summary can reduce via `.max()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RangeStatus {
    Fresh,     // content unchanged since anchor
    Moved,     // location changed (rename / line shift), bytes identical
    Changed,   // content differs from anchored bytes (partial or total)
    Orphaned,  // anchor commit no longer reachable
}

#[derive(Clone, Debug)]
pub struct RangeLocation {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub blob: String,
}

#[derive(Clone, Debug)]
pub struct RangeResolved {
    pub range_id: String,
    pub anchor_sha: String,
    pub anchored: RangeLocation,
    pub current: Option<RangeLocation>,
    pub status: RangeStatus,
}

#[derive(Clone, Debug)]
pub struct MeshResolved {
    pub name: String,
    pub message: String,
    /// One resolved entry per Range id in the Mesh, in the Mesh's
    /// stored order. Each carries its own status; the Mesh does not
    /// aggregate them.
    pub ranges: Vec<RangeResolved>,
}
```

## 5. Resolution and staleness

### 5.1 Locate

The resolver runs once per range, using the mesh's `copy-detection` and
`ignore-whitespace` settings from the `config` file in the mesh commit's tree:

```
git log -L <start>,<end>:<path> [--follow -M -C...] <anchor_sha>..HEAD
```

`git log -L` is git's line-range history walker. It performs the
diff-hunk arithmetic, handles renames when `--follow` / `-M` are on,
and detects copies per the mesh's `copy-detection` setting. Output is
the current `(path, start, end)` plus the blob OID of the file at HEAD.

If the walker reports the range as deleted at some commit with no
surviving successor under the configured copy detection, the range's
current location is `None` and its status is `Changed` — the full
removal is surfaced as a diff against `/dev/null`.

### 5.2 Compare

Anchored bytes and current bytes are extracted from their blobs:

```
git cat-file -p <blob> | sed -n '<start>,<end>p'
```

Equality (byte-for-byte, or normalized if the mesh's `ignore-whitespace`
is `true`) determines the range's status. If the location changed but bytes are
identical, the status is `Moved`. If the bytes differ in any way —
whether one line or all lines — the status is `Changed`.

Rich status — which lines changed, which commit introduced the change —
is produced by the reporter running `git blame -w -C` over the resolved
range; blame options are internal to the reporter and not part of the
stored shape.

### 5.3 Status values

| Status | Meaning |
|---|---|
| `FRESH` | Current bytes equal anchored bytes. |
| `MOVED` | Bytes equal; `(path, start, end)` changed. |
| `CHANGED` | Anchored bytes differ from current bytes, including complete deletion. |
| `ORPHANED` | `anchor_sha` is not reachable from any ref. |

`CHANGED` covers any range whose anchored bytes differ from current
bytes, including complete deletion. The diff output conveys the degree
of change directly; no threshold policy is needed.

### 5.4 Mesh reporting

A Mesh does not have a single status. `status`/`show` emit the per-Range
status for each Range in the Mesh. The declaration order of `RangeStatus`
(`Fresh` < `Moved` < `Changed` < `Orphaned`) is provided as a convention
so callers that need a summary can apply their own "worst wins" reduction.

## 6. Operations

All writes are atomic. Range writes are a blob write + reference update.
Mesh writes are a blob write (`ranges`), a blob write (`config`), a tree
write, a commit write (whose message is the Mesh's message) with the
prior Mesh tip as parent, and a reference update with an expected
previous value (compare-and-swap). If the CAS fails because another
client advanced the Mesh concurrently, the caller retries with the new
tip as parent.

### 6.1 Create a Range

At `git mesh commit` time, each staged `add` is resolved into a Range
record and written to `refs/ranges/v1/<id>`:

1. Resolve the anchor SHA (`--at` value, default HEAD).
2. Look up `path` in the anchor commit's tree; error if not found.
3. Validate that `start` and `end` are within the file's line count.
4. Record the blob OID of `path` at the anchor commit.
5. Serialize the `Range` record in the format described in §4.1.
6. Write the serialized text as a git blob (`git hash-object -w`).
7. Create `refs/ranges/v1/<uuid>` pointing at that blob
   (`update-ref`, fail if already exists).

Range ids are UUIDs. The ref must not already exist; collisions would
only arise from retried writes on content-identical blobs, which are
safe to ignore.

### 6.2 Commit a change to a Mesh

`git mesh commit` reads the staging area, validates all operations, then
writes a single mesh commit:

**Validation (pure, no writes):**

1. Verify the mesh name is not reserved.
2. Load the current `ranges` list from the existing mesh commit, or
   start with an empty list for a new mesh.
3. For each staged `remove`: confirm `(path, start, end)` exists in the
   current list; error with the offending range if not.
4. For each staged `add`: confirm `(path, start, end)` does not collide
   with the post-remove list; error with the offending range if it does.
5. Verify the staging area is non-empty; error if nothing is staged.

The remove-then-add ordering means a re-anchor (stage `rm X`, then
`add X` at a new commit) is always valid — the pair is absent at the
moment the add is validated.

**Write (after validation passes):**

1. Resolve each staged `add` into a Range record and write it to
   `refs/ranges/v1/<uuid>` (see §6.1).
2. Apply removes: drop the matching Range ids from the list.
3. Apply adds: append the new Range ids.
4. Sort and deduplicate the list.
5. Write the `ranges` blob (one id per line) and the `config` blob
   (final staged config values merged with the previous committed
   config).
6. Write a tree containing both blobs.
7. Write a commit with the tree, the staged message, and the prior mesh
   tip as parent (none for a new mesh).
8. Atomically update `refs/meshes/v1/<name>` via CAS; retry if another
   client advanced the tip concurrently.
9. Delete all `.git/mesh/staging/<name>*` files.

**Errors.** A single invalid operation aborts the call before any object
is written. All-or-nothing.

### 6.3 Staging area

`git mesh add` and `git mesh rm` do not write mesh commits directly.
They accumulate operations in a staging area at
`.git/mesh/staging/<name>`. `git mesh commit` resolves and finalizes.

**On-disk format:**

Files under `.git/mesh/staging/` per mesh:

- `<name>` — pending operations, one per line.
- `<name>.msg` — the staged message, set via `git mesh message`. Read
  verbatim at commit time. Supports multi-line messages.
- `<name>.<N>` — full file bytes captured at `git mesh add` time for the
  `add` operation on line `N` of the operations file. One sidecar file
  per `add` line; no sidecar for `remove` or `config` lines.

**Operation format:**

```
add <path>#L<start>-L<end>
remove <path>#L<start>-L<end>
config <key> <value>
```

- `add` lines record the range address only. The full file bytes are
  stored in the corresponding `<name>.<N>` sidecar file, written from
  the working tree at the time `git mesh add` was run. The line number
  `N` is the 1-based index of the `add` line in the operations file,
  providing a stable, unique key per staged add.
- `remove` lines carry only the range; no bytes stored (removes are
  matched by `(path, start, end)`, not validated against current bytes).
- `config` lines set a mesh-level resolver option. Last write wins per
  key; at commit time the final value for each key is written into the
  mesh commit's `config` file, replacing the previous committed value.

```
# .git/mesh/staging/frontend-backend-sync       (operations file)
add src/Button.tsx#L42-L50                      (line 1)
add server/routes.ts#L13-L34                    (line 2)
remove src/old.ts#L1-L10                        (line 3 — no sidecar)
config copy-detection any-file-in-commit        (line 4 — no sidecar)

# .git/mesh/staging/frontend-backend-sync.1     (bytes of src/Button.tsx at add time)
# .git/mesh/staging/frontend-backend-sync.2     (bytes of server/routes.ts at add time)

# .git/mesh/staging/frontend-backend-sync.msg
ABC-123: front-end components reflect back-end endpoints

Owner: team-billing
```

Transient local state — follows git's convention for working files
(`.git/rebase-merge/`, etc.). Does not sync. `git mesh commit` and
`git mesh restore` delete all `<name>*` files under
`.git/mesh/staging/` when clearing the staging area.

**Validation against the working tree.** `git mesh status <name>` and a
pre-commit hook both compare staged ranges against the current working
tree. For each staged `add` at line `N`, the tool extracts the range
bytes from the `<name>.<N>` sidecar file and compares them against the
same byte range read directly from the file on disk. If the bytes
differ, the user is alerted before `git commit` runs.

Error format (working tree check):

```
warning: staged range has uncommitted changes

--- <path>#L<start>-L<end> (staged)
+++ <path>#L<start>-L<end> (working tree)
@@ ... @@
  <unified diff of the affected range>
```

**Suggested pre-commit hook** (installed at `.git/hooks/pre-commit`):

```bash
#!/bin/sh
git mesh status --check
```

`git mesh status --check` exits non-zero if any staged range differs
from the working tree, printing the diff for each affected range. The
pre-commit hook surfaces this before the source commit lands, giving the
developer a chance to update the staged range or amend the working tree
change.

**Validation at commit time.** For each staged `add` at line `N`, the
tool extracts the range bytes from the `<name>.<N>` sidecar file and
compares them against the same byte range from the HEAD blob. If the
bytes differ at all, the commit is aborted. Nothing is written until all
staged ranges pass. Fail closed. This is a second line of defence — the
pre-commit hook should catch drift first, but the commit-time check is
authoritative.

Error format (commit time):

```
error: staged mesh '<name>' is stale

--- <path>#L<start>-L<end> (staged)
+++ <path>#L<start'>-L<end'> (HEAD)
@@ ... @@
  <unified diff of the affected range>
```

### 6.4 Status

`git mesh status <name>` reports the pending staging area state alongside
the committed mesh state. Fast — no resolver, no `git log -L`, no network.
Analogous to `git status`.

Ranges with no staged operations are not shown. Working tree drift for
each staged add is detected by comparing the `<name>.<N>` sidecar bytes
against the file on disk.

**Default output.**

```
mesh frontend-backend-sync
commit e0f92a3b8c1d0f5e6a7b2c4d9e1f0a3b8c1d0f5e
Author: John Wehr <john@example.com>
Date:   Wed Apr 22 11:14:03 2026 +0000

    ABC-123: front-end components reflect back-end endpoints

Staged changes:

  add     src/Button.tsx#L42-L50
  add     server/routes.ts#L13-L34
  remove  src/old.ts#L1-L10
  config  copy-detection any-file-in-commit

Staged message:

  ABC-123: front-end components reflect back-end endpoints

Working tree drift:

  src/Button.tsx#L42-L50

--- src/Button.tsx#L42-L50 (staged)
+++ src/Button.tsx#L42-L50 (working tree)
@@ -42,3 +42,4 @@
  const x = 1;
+const y = 2;
```

**Output conventions.**

- **Mesh header** mirrors `git mesh <name>` — commit metadata and
  message from the current tip.
- **Staged changes** lists each pending operation in order: `add`,
  `remove`, and `config` lines from the operations file.
- **Staged message** shown only if a `.msg` file is present.
- **Working tree drift** shown only for staged adds where the sidecar
  bytes differ from the file on disk. Flat diff, no indentation,
  matching `git diff` output directly.

`git mesh status --check` suppresses human output and exits non-zero if
any staged range differs from the working tree. Used by the pre-commit
hook.

### 6.5 Show and stale

Two read operations with different costs:

- **`show(name)`** — read-only and fast. Loads `refs/meshes/v1/<name>`,
  the `ranges` blob from the commit's tree, and each referenced Range
  blob. Returns the Mesh's stored state as-is: commit metadata, message,
  and per-Range `(anchor_sha, path, start, end, blob)`. No resolver, no
  `git log -L`, no byte comparison. This is what `git mesh <name>`
  invokes.
- **`stale(name)`** — runs the resolver for every Range and produces a
  `MeshResolved` with per-Range status. Exposed as `git mesh stale
  <name>`. Computationally heavier; users invoke it when they want the
  drift picture, not just the list.

Neither has side effects; the stored Mesh is never modified.

### 6.6 History and revert

- **History:** `git log refs/meshes/v1/<name>` walks every prior state
  of the Mesh. `git log -p` shows the `ranges` diff per edit; each
  commit's message is the Mesh's message at that point.
- **Revert:** `git mesh revert <name> <commit-ish>` rolls the Mesh
  forward to the state at `<commit-ish>` by writing a new commit whose
  tree matches it. History is never rewritten; the restoration is a
  normal fast-forward append.

### 6.7 Doctor

`git mesh doctor` audits the local mesh setup and reports actionable
findings. Checks include:

- **Missing post-commit hook.** Detects the absence of a `post-commit`
  hook that auto-commits pending staged meshes after a source commit.
  Prints the suggested hook and the install path.

  **Suggested post-commit hook** (installed at `.git/hooks/post-commit`):

  ```bash
  #!/bin/sh
  git mesh commit
  ```

  With no `<name>`, `git mesh commit` commits every mesh that has a
  non-empty staging area.

- **Missing pre-commit hook.** Detects the absence of a `pre-commit`
  hook that checks staged ranges for working tree drift before a source
  commit lands.

  **Suggested pre-commit hook** (installed at `.git/hooks/pre-commit`):

  ```bash
  #!/bin/sh
  git mesh status --check
  ```

- **Stale or corrupt staging area files.** Checks
  `.git/mesh/staging/` for malformed operation lines, missing sidecar
  files, and orphaned sidecar files (sidecar with no corresponding `add`
  line). Warns on each finding with the file and line number.

### 6.8 Structural operations

These mirror git's own file-level commands and are surfaced as
subcommands rather than flags — consistent with git's convention for
destructive or ref-shape-changing operations.

- **`git mesh delete <name>`** — delete the Mesh's ref:
  `git update-ref -d refs/meshes/v1/<name>`. Reachable commits stay in
  the object database until `git gc` collects them; the ref is gone
  immediately.
- **`git mesh mv <old> <new>`** — rename:
  `git update-ref refs/meshes/v1/<new> <commit>` followed by
  `git update-ref -d refs/meshes/v1/<old>`, both atomic. If you want an
  alias, leave both refs in place — `git mesh mv --keep <old> <new>` is
  a convenience that omits the delete step.
- **`git mesh restore <name>`** — clear the staging area for `<name>`:
  deletes all `.git/mesh/staging/<name>*` files (operations file, `.msg`,
  and all `.<N>` sidecar files). Analogous to `git restore --staged` on
  all files.
- **Delete a Range blob** is never done directly; Ranges are referenced
  by Mesh commits and tracked by git's reachability. Once no Mesh
  references a Range, `git gc` collects its blob. If a Mesh somehow
  references a missing Range id (e.g. a partial clone), the resolver
  reports that Range as `ORPHANED`.

## 7. Sync

### 7.1 Refspec

```ini
[remote "origin"]
    fetch = +refs/ranges/*:refs/ranges/*
    push  = +refs/ranges/*:refs/ranges/*
    fetch = +refs/meshes/*:refs/meshes/*
    push  = +refs/meshes/*:refs/meshes/*
```

The `*` matches every schema version. To pin a client to a single
version, narrow the refspec to `refs/ranges/v1/*` and
`refs/meshes/v1/*`.

Refspecs are configured **lazily**: the first `fetch` or `push` that
touches a remote adds missing refspec lines idempotently via
`git config --add`. There is no separate `init` step.

### 7.2 Remote visibility

Most hosts (GitHub, GitLab, Bitbucket) accept arbitrary `refs/*`
namespaces over the normal git protocol, but their web UIs do not render
them. Use `git ls-remote origin 'refs/meshes/*'` to list them.
Branch-protection rules do not apply to custom refs; the tool's write
path is the integrity boundary.

## 8. Merge semantics

### 8.1 Divergence

Two clients both edit the same Mesh on different local branches. Each
produces a new Mesh commit whose parent is the shared tip. First push
wins; the second push to `refs/meshes/v1/<name>` is rejected as
non-fast-forward, identical to a branch.

### 8.2 Three-way merge

`git merge` (or `git merge-tree` for headless resolution) performs the
standard three-way merge on the Mesh commits. Because the `ranges` file
is canonicalized — sorted, deduplicated, one id per line, trailing
newline — independent edits produce non-conflicting diffs. The message
lives on the commit object, so message edits are a normal
commit-message merge. Conflicts arise exactly when two branches
disagree on the same piece of state:

- Two branches re-anchored the same range to different new locations
  (producing different replacement Range ids).
- Two branches edited the message to different text.
- Two branches added distinct new Ranges (usually clean; the merge
  interleaves them into the sorted list).

### 8.3 Resolving

The merge commit's tree (its `ranges` file) and message record the
resolution. Downstream readers see the same state. Since the prior
commits remain in history, no data is lost; a reverted decision can be
reinstated by checking out the earlier commit and re-applying.

## 9. Repository visibility

Mesh refs live outside `refs/heads/*`, so they do not appear as branches
in `git branch`, `git branch -a`, or a host's branch-list UI. They **do**
appear in `--all`-style traversals.

| Command / view | Shows Mesh commits? |
|---|---|
| `git branch`, `git branch -a` | No |
| `git log`, `git log <branch>` | No |
| `git log --branches` | No |
| `git log --all` | Yes |
| `gitk --all`, IDE "all branches" views | Yes |
| `git for-each-ref` | Yes (plumbing) |
| GitHub / GitLab branch lists | No |
| `git fetch --prune` | Only refs matching the configured refspec |

There is no git config that silently excludes a custom namespace from
`--all`. Recommended mitigations:

- A scoped history alias:
  `git config alias.hist 'log --graph --branches --remotes --tags'`.
- For explicit `--all` users: `git log --all --exclude=refs/meshes/*`.
- To drop ref-name decoration noise:
  `git config log.excludeDecoration refs/meshes/*`.

Range refs point at blobs, not commits, so no `--log`-family command
traverses them regardless of flags.

## 10. CLI reference

### 10.1 Synopsis

```
git mesh                                # list all meshes
git mesh <name>                         # show the named mesh (always read)
git mesh <subcommand> [<args>]          # everything else
```

### 10.2 Commands

```
Reading
  git mesh                              # list every mesh (like `git branch`)
  git mesh <name>                       # show the mesh (like `git show`)
  git mesh <name> --oneline             # one line per Range, no commit header
  git mesh <name> --format=<fmt>        # format-string override
  git mesh <name> --no-abbrev           # full 40-char shas
  git mesh <name> --at <commit-ish>     # show state at a past revision
  git mesh <name> --log [--oneline] [--limit <n>]
  git mesh <name> --diff <rev>..<rev>   # compare two states
  git mesh stale [<name>]               # run the resolver, report drift

Staging (write to staging area; no mesh commit yet)
  git mesh add <name> <range>...        # stage ranges to add
  git mesh rm <name> <range>...         # stage ranges to remove
  git mesh message <name>               # set the staged message
    [-m <msg> | -F <file> | --edit]

Committing (resolve staged operations and write a mesh commit)
  git mesh commit [<name>]              # commit all staged meshes, or just <name>
    [--at <commit-ish>]                 # resolve anchors against this commit (default: HEAD)

Structural
  git mesh restore <name>               # clear the staging area
  git mesh revert <name> <commit-ish>   # fast-forward to a past state
                                        # (new commit whose tree matches
                                        #  <commit-ish>; no history rewrite)
  git mesh delete <name>                # delete the mesh's ref entirely
  git mesh mv <old> <new>               # rename
  git mesh mv --keep <old> <new>        # alias (keep both refs)

Configuration
  git mesh config <name> <key> <value>  # stage a mesh-level resolver option

Maintenance
  git mesh fetch [<remote>]
  git mesh push  [<remote>]             # auto-configures refspec on first run
  git mesh doctor
  git mesh status <name>                # show staging area state (see §6.4)
  git mesh status --check               # exit non-zero if any staged range has drifted
```

**Staging workflow.** `git mesh add`, `git mesh rm`, and `git mesh message`
write to the staging area; `git mesh commit` resolves and finalizes. This
mirrors git's own `git add` / `git commit` separation. Multiple add and rm
invocations accumulate in the staging area; one `git mesh commit` lands
them all in a single mesh commit. With no `<name>`, `git mesh commit`
commits every mesh that has a non-empty staging area — this is how the
post-commit hook works.

**Semantics of `git mesh commit`:**

- *Create:* first commit on a fresh name creates the ref.
- *Re-anchor:* `git mesh rm <name> <range>` then
  `git mesh add <name> <range>` (same location) removes the existing
  Range and adds a fresh one anchored at `--at` (default HEAD). This is
  the only legitimate way to re-add a currently-present `(path, start, end)`.
- *Empty staging area is an error:* `git mesh commit` with nothing staged
  does nothing.
- *Collision errors are atomic:* an add that duplicates an existing
  `(path, start, end)` errors before any object is written. All-or-nothing.

**Reserved names.** Mesh names cannot collide with subcommands:
`add`, `rm`, `commit`, `message`, `restore`, `revert`, `delete`, `mv`,
`stale`, `fetch`, `push`, `doctor`, `log`, `config`, `status`, `help`.

### 10.3 Range syntax

```
Range      <path>#L<start>-L<end>
           e.g. src/Button.tsx#L42-L50
```

- The `#L<start>-L<end>` form matches GitHub's URL-fragment convention,
  so a range pasted from a browser URL works verbatim.
- Within a Mesh, every Range has a unique `(path, start, end)`, so a
  range address always identifies exactly one Range.

### 10.4 Read output format

`git mesh <name>` follows `git show` conventions: human-readable header,
indented commit message, labeled section for the structured body.

```
mesh <name>
commit <full-sha>
Author: <name> <<email>>
Date:   <commit date>

    <subject>

    <body>

Ranges (<N>):
    <short-sha>  <path>#L<start>-L<end>
    <short-sha>  <path>#L<start>-L<end>
    ...
```

- **Header line** is `mesh <name>`, mirroring `git show`'s `commit <sha>`.
- **`commit`, `Author`, `Date`** are taken from the Mesh's tip commit
  object. Full sha by default for `commit`; abbreviated for Range shas.
- **Message** is indented four spaces, the same rendering `git show`
  uses.
- **Ranges section** lists `<anchor-sha> <path>#L<start>-L<end>`, one
  per line. Order is the Mesh's canonical (sorted) order; stable across
  runs.
- `--oneline` drops the header and prints only the Ranges section body.
  `--format=<fmt>` takes a git-log-style format string. `--no-abbrev`
  shows full 40-char shas.

#### `git mesh stale`

`git mesh stale` resolves each Range in a Mesh and reports drift with
culprit-commit attribution.

**Staleness rule.** Any status other than `FRESH` is stale. `MOVED`
counts — if a range shifted, a human should at least glance at it even
though the bytes are identical. No gradation flag; one rule.

**Synopsis.**

```
git mesh stale [<name>]
    [--format=human|porcelain|json|junit|github-actions]
    [--exit-code]                   # non-zero if any Range is not FRESH
    [--oneline | --stat | --patch]  # detail level
    [--since <commit-ish>]          # only Ranges anchored at or after this commit
```

With `<name>`, reports one Mesh. Without, reports every Mesh in the
repo, worst-first. Reads are purely local; users run `git mesh fetch`
first if they want remote state.

**Default (human) output.**

```
mesh frontend-backend-sync
commit e0f92a3b8c1d0f5e6a7b2c4d9e1f0a3b8c1d0f5e
Author: John Wehr <john@example.com>
Date:   Wed Apr 22 11:14:03 2026 +0000

    ABC-123: front-end components reflect back-end endpoints

3 stale of 5 ranges:

Orphaned ranges:

  src/auth.ts#L1-L20
  anchor e3f4a5b6 is unreachable — run `git fetch` or check for a force-push

Changed ranges:

  server/routes.ts#L13-L34
  caused by 4e8b2c1a refactor: extract session helper  (2 days ago)

--- server/routes.ts#L13-L34 (anchored)
+++ server/routes.ts#L15-L36 (HEAD)
@@ -13,5 +15,6 @@
  function handleClick(event) {
-    const session = getSession();
+    const session = getSession(event.target);
  }

  src/old.ts#L1-L10
  caused by 7f1a2b3c refactor: remove legacy code  (1 week ago)

--- src/old.ts#L1-L10 (anchored)
+++ /dev/null
@@ -1,10 +0,0 @@
-function legacyHandler() {
-    res.send(deprecated());
-}

Moved ranges:

  server/schema.ts#L20-L27 → server/schema.ts#L24-L31
```

**Output conventions.**

- **Summary line first** (`N stale of M ranges`), mirroring `git status`'s
  header-then-body cadence.
- **Grouped by status, worst-first:** `Orphaned ranges` → `Changed ranges`
  → `Moved ranges`. Fresh ranges are not shown.
- **Flat diffs** — no indentation, matching `git diff` output directly.
- **Culprit commit attribution** on `Changed` ranges: the short sha and
  subject of the commit that introduced the drift, found via `git log -L`.
  Not shown on `Moved` — there is no meaningful culprit for a pure
  location shift.
- **Deletion is a diff** — a range that no longer exists is shown as a
  removal against `/dev/null`, not a special case.

**Exit codes.**

- `0` — no stale Ranges in scope, or `--exit-code` not passed.
- `1` — at least one Range is not `FRESH`; only returned when
  `--exit-code` is passed.
- `2` — tool error (missing repo, corrupt ref, etc.). Distinct from `1`
  so CI can tell "should fail the build" from "tool is broken."

**Machine-readable formats.**

- **`--format=porcelain`** — stable one-line-per-finding schema for
  shell pipelines.
- **`--format=json`** — versioned from v1: `{"version": 1, "mesh",
  "commit", "ranges": [...]}`. Each Range entry mirrors the LSP
  `Diagnostic` shape (`severity`, `range`, `message`, `code`,
  `data.culprit`), so editor plugins consume it directly and surface
  squiggles.
- **`--format=junit`** — generic CI integration.
- **`--format=github-actions`** — `::warning file=<path>,line=<n>::<msg>`
  annotations that land on the offending line in PR reviews.

**Build order (phased delivery).**

1. **Exit-code discipline + `--format=porcelain` and `--format=json`.**
   Without these, no automation can use the command. Ship with v1.
2. **Culprit attribution.** Turns "something is wrong" into "here's what
   and here's why."
3. **`--format=github-actions`, `--format=junit`, `--since`.** Additive
   CI ergonomics once the core is solid.

Status tokens are the `RangeStatus` enum values from §4.4 (`FRESH`,
`MOVED`, `CHANGED`, `ORPHANED`).

### 10.5 Config

The tool reads these keys from `git config`:

```
mesh.defaultRemote  string, default "origin". Used by `git mesh fetch`
                    and `git mesh push` when no remote is supplied.
```

Per-mesh resolver options (`copy-detection`, `ignore-whitespace`) are
staged via `git mesh config <name> <key> <value>` and committed with
the next `git mesh commit`. They are stored in the mesh commit's `config`
file (see §4.3) and sync with the mesh.

**Reads never touch the network.** `git mesh <name>`, `git mesh stale`,
and every other read operation work entirely against local object
storage. To incorporate remote updates, run `git mesh fetch` explicitly
— the same discipline git applies to `git log`, `git show`, `git
status`, and `git blame`.

### 10.6 Examples

```
# Stage ranges — no commit yet
$ git mesh add frontend-backend-sync \
      src/Button.tsx#L42-L50 \
      server/routes.ts#L13-L34 \
      src/types.ts#L8-L15 \
      server/schema.ts#L20-L27

# Set the message separately
$ git mesh message frontend-backend-sync -m "ABC-123: front-end components reflect these back-end endpoints

Owner: team-billing"

# Commit: resolves anchors against HEAD, writes mesh commit, clears staging
$ git mesh commit frontend-backend-sync
created refs/meshes/v1/frontend-backend-sync

# Show: fast read, no resolver. Header + message + Range list with anchor shas.
$ git mesh frontend-backend-sync
mesh frontend-backend-sync
commit e0f92a3b8c1d0f5e6a7b2c4d9e1f0a3b8c1d0f5e
Author: John Wehr <john@example.com>
Date:   Wed Apr 22 11:14:03 2026 +0000

    ABC-123: front-end components reflect these back-end endpoints

    Owner: team-billing

Ranges (4):
    63d239a4  src/Button.tsx#L42-L50
    63d239a4  src/types.ts#L8-L15
    63d239a4  server/routes.ts#L13-L34
    63d239a4  server/schema.ts#L20-L27

# Stale: runs the resolver for every Range. Shows drift.
$ git mesh stale frontend-backend-sync
...

# Reconcile a drifted Range: remove old location, add new location.
$ git mesh rm frontend-backend-sync server/routes.ts#L13-L34
$ git mesh add frontend-backend-sync server/routes.ts#L15-L36
$ git mesh message frontend-backend-sync -m "routes.ts refactor"
$ git mesh commit frontend-backend-sync

# Stage a config change and commit it with the next mesh commit
$ git mesh config frontend-backend-sync copy-detection any-file-in-commit
$ git mesh commit frontend-backend-sync

# Anchor against a specific commit rather than HEAD
$ git mesh commit frontend-backend-sync --at HEAD~1

# Publish. Refspec is configured automatically on first push.
$ git mesh push
configuring refs/{ranges,meshes}/* on origin... done
pushed 1 mesh, 4 ranges

# Automatic commit via post-commit hook (see §6.3)
$ git mesh add frontend-backend-sync \
      src/Button.tsx#L42-L50 \
      server/routes.ts#L13-L34
$ git mesh message frontend-backend-sync -m "ABC-123: wire Button to routes"
$ git commit -m "ABC-123: wire Button to routes"
# post-commit hook fires `git mesh commit` for all staged meshes
```

## 11. Appendix: why these primitives

- **Ranges are blobs because they are immutable and content-addressed.**
  Two identical anchors share storage automatically. The ref is a stable
  name for a piece of immutable content; "editing" a Range means
  creating a new one.
- **Meshes are commits because they are mutable with history.** A commit
  ref (name + parent chain) is exactly the shape git uses for branches,
  and the same primitive gives edit history, per-edit author/date/message,
  and native three-way merges for concurrent edits — all without any
  custom merge logic.
- **A Mesh is a set of ranges, not pairs.** The relationship is named by
  the mesh; no pairwise structure needs to be stored or maintained. If
  two independent relationships need tracking, that is two meshes.
- **A Mesh's name is the ref name, branch-style.** No separate
  name-to-id mapping, no UUID indirection. Renames, aliases, collision
  detection, and sync all reuse git's existing ref machinery.
- **On-disk records are commit-object-style text, not JSON.** Header
  lines for structured fields, TAB before any field that may contain
  spaces (paths). Typical Range is ~80 bytes; a Mesh's message is
  stored once as the commit message rather than duplicated into the tree.
- **Mesh config is stored in the commit tree, not as local state.** It
  syncs with the mesh, its history is visible in `git log -p`, and it
  is staged and committed like range changes — last write wins per key.
- **Staleness is computed, not stored, because the repo is the only
  authoritative source of whether content has changed.** A cached status
  field would either lag the truth or require eager invalidation on every
  commit; resolving on demand costs a few cheap git plumbing calls per
  member and is always correct.
- **`CHANGED` replaces `MODIFIED`, `REWRITTEN`, and `MISSING`.** The
  degree of change and whether a range was deleted are both conveyed
  directly by the diff output. Threshold-based classifications
  (`MODIFIED` vs `REWRITTEN`) encode policy in the status rather than
  leaving it to the reader.
- **Versioned ref namespaces (`v1`) are used instead of a payload
  `version` field so schema recognition happens at the ref level, not
  the blob level.** Readers can enumerate and filter by shape without
  opening any object, and future shapes coexist with old ones without
  migration.
