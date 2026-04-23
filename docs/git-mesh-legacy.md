# Git Mesh

A two-tier system for attaching tracked, updatable metadata to portions of
code in a git repository. Data lives in git's object database under custom
refs — nothing is written to the working tree, and the system has no
sidecar database.

## 1. Overview

There are exactly two primitives:

- **Link** — an immutable anchor to an exact range of bytes in a file at a
  specific commit. Links are content-addressed, shared, and cheap.
- **Mesh** — a mutable, commit-backed record that groups a set of Link
  references together under a free-text message. Meshes are how humans
  name and describe the relationships between anchored ranges, and they
  evolve over time as the code evolves.

Staleness — whether a Link's anchored bytes still exist somewhere in the
current tree, and whether a Mesh's members are collectively up-to-date —
is **always computed**, never stored. The repo's history is the only
source of truth; the stored records are minimal.

## 2. Concepts

### 2.1 Link

A Link is an association between **two** line ranges, both anchored at
the same commit. The Link carries one `anchor_sha`; each side carries
its own `(path, range, blob)`. Given a Link, the tool can independently
ask each side:

- *Where is this range now?* — `git log -L` walks forward from the
  shared anchor commit, following the range through diff hunks, renames,
  and copies.
- *Is the content still the same?* — extract the anchored bytes from the
  side's anchor blob; extract the bytes at the resolved location;
  compare.

A Link's status is the worse of its two sides. Links are immutable:
once written, the ref points at its blob forever (or until deleted).

### 2.2 Mesh

A Mesh is a named set of Link references plus a free-text message. It
expresses "these anchored ranges belong together, and here is why." A
Mesh has exactly three pieces of state:

- **name** — the identity of the Mesh, carried by the ref name
  (`refs/meshes/v1/<name>`). Mirrors branches: the name *is* the mesh,
  and git's ref machinery handles collisions, renames, atomic updates,
  and sync.
- **links** — a sorted, deduplicated set of Link ids. Each id names a
  Link currently considered part of the relationship. Stored as one
  line per id in a single file inside the Mesh commit's tree.
- **message** — a git-commit-message-style string describing the
  relationship. This *is* the commit's message; it is not duplicated
  anywhere in the tree.

A Mesh is mutable: edits write new commits on the Mesh's ref; the parent
chain records every past state. Per-edit history (which Link was added,
removed, or swapped in for another) is recovered by diffing a commit's
tree against its parent; it is not denormalized into the stored record.

### 2.3 Staleness

Staleness is a per-Link property, always computed on query:

- **Side status** — for each side: byte equality (modulo
  `ignore_whitespace`) between the anchored bytes and the bytes at the
  resolved location.
- **Link status** — the worse of its two side statuses.
- **Mesh** has no single aggregate status. `status`/`show` report each
  Link's status individually; callers that want a one-line summary
  decide their own aggregation rule (e.g. "any Link not `Fresh`" →
  needs attention).

The stored records never carry status fields. Every query recomputes
against HEAD, so the answer always matches the repo as it is now.

## 3. Storage model

### 3.1 Ref layout

```
refs/links/v1/<linkId>     →   blob      →   Link record (text)
refs/meshes/v1/<name>      →   commit    →   tree  →  links  (text file)
                                   │
                                   └── parent: previous Mesh commit (or none)
```

- Link refs point directly at a content-addressed text blob in a
  commit-object-style format (see §4.1). Identical Link payloads share
  a blob in the object database automatically.
- Mesh refs are named by the user. The commit's tree contains one
  file, `links`, with one Link id per line. The commit's **message**
  is the Mesh's message — no duplication in the tree. The commit's
  parent pointers form the Mesh's edit history.

### 3.2 Versioned namespace

The `v1` segment encodes the schema version of the stored JSON. A reader
can enumerate only shapes it understands (`git for-each-ref refs/links/v1/`,
likewise for meshes) without opening any blob. Refspecs can filter by
version. A future breaking change introduces `refs/links/v2/*` and
`refs/meshes/v2/*`; v1 records remain readable under their own namespace
indefinitely. There is no implicit migration.

### 3.3 Why these git objects

- **Link → blob (text).** Immutable, content-addressed, dedup-friendly.
  Writing is `git hash-object -w`; the id lives in the ref name. The
  on-disk format mirrors git's own commit/tag header style — `key SP
  value\n` lines, TAB-separated paths — so it's readable, compact, and
  parseable line-by-line.
- **Mesh → commit.** Mutable tip with a parent chain is exactly what
  git branches are; the same primitive gives edit history for free,
  captures author/date per edit, carries the Mesh's message as the
  commit message, and supports real three-way merges when two branches
  edit the same Mesh concurrently. The user-facing name is the ref
  name, just like a branch.

### 3.4 Name and id format

Both `<linkId>` and `<name>` must be ref-legal path components: no
slashes, no whitespace, no control characters, and not a leading `-`.

- `<linkId>` is always a UUID. Link refs are internal; users never see
  or type them.
- `<name>` is user-chosen on `create`. It is the Mesh's only identity,
  and the same name cannot be used by two Meshes (ref-level collision,
  caught by `update-ref` CAS). Names follow the same rules git applies
  to branch names.

## 4. Data shapes

All types below describe the v1 on-disk shape. Every field is required;
defaults are applied at creation time so stored records fully
self-describe their resolver behaviour. JSON field names are camelCase
(via `serde(rename_all = "camelCase")`); Rust fields are snake_case.

### 4.1 Link

**On-disk format** (commit-object-style text, stored as the blob at
`refs/links/v1/<linkId>`):

```
anchor <sha>
created <iso-8601>
side <start> <end> <blob> <copy-detection> <ignore-whitespace>\t<path>
side <start> <end> <blob> <copy-detection> <ignore-whitespace>\t<path>
```

- Headers are `key SP value\n`. Unknown headers are tolerated (future
  additive extensions don't break v1 readers).
- Each `side` line carries six space-separated fields, then a `\t`, then
  the path. Paths may contain spaces; no other field may, so the TAB
  unambiguously terminates the field block — the same dodge git uses
  in tree entries.
- The two `side` lines are sorted on write so the pair is effectively
  unordered.
- Trailing newline; no blank lines.

Typical size is ~150 bytes.

**Rust types** (ser/de is a hand-written parser, not serde-JSON):

```rust
use serde::{Deserialize, Serialize};

/// In-memory representation of the Link record stored at
/// refs/links/v1/<linkId>. The id itself is the ref name suffix and is
/// not repeated in the blob.
#[derive(Clone, Debug)]
pub struct Link {
    /// Commit both sides were anchored to at creation.
    pub anchor_sha: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// The two anchored ranges this Link associates. Canonicalized on
    /// write by sorting so the pair is effectively unordered.
    pub sides: [LinkSide; 2],
}

/// One anchored range. The anchor commit is shared across both sides
/// of the Link; each side carries its own path, range, blob, and
/// per-side resolver options.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LinkSide {
    /// File path at the anchor commit.
    pub path: String,
    /// 1-based, inclusive line range.
    pub start: u32,
    pub end: u32,
    /// Blob OID of `path` at the Link's `anchor_sha`. Freezes the
    /// exact anchored bytes and keeps the side verifiable even if
    /// `anchor_sha` becomes unreachable.
    pub blob: String,
    /// How aggressively `git log -L` follows the range across files
    /// when resolving this side's current location.
    pub copy_detection: CopyDetection,
    /// Whether whitespace-only differences count as a content change.
    pub ignore_whitespace: bool,
}

/// -C levels for `git log -L` copy detection. Serialized as the
/// kebab-case variant name: `off`, `same-commit`, `any-file-in-commit`,
/// `any-file-in-repo`.
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

pub const DEFAULT_COPY_DETECTION: CopyDetection = CopyDetection::SameCommit;
pub const DEFAULT_IGNORE_WHITESPACE: bool = true;
```

### 4.2 Mesh

**On-disk shape.** A Mesh is a commit whose tree contains a single
file, `links`, and whose commit message is the Mesh's message. No
`mesh.json`, no tree-level message duplication.

```
refs/meshes/v1/<name>
└── commit
    ├── message: "<subject>\n\n<body>"   ← the Mesh's message
    └── tree
        └── links                        ← text file, one Link id per line
```

The `links` file:

```
0a1b2c3d4e5f...
4d5e6f7a8b9c...
8e9f0a1b2c3d...
```

- One Link id per line.
- Sorted ascending; duplicates removed on write.
- Trailing newline; no blank lines.

**Rust types** (assembled on read from the commit and the `links` blob;
serialized back the same way on write):

```rust
#[derive(Clone, Debug)]
pub struct Mesh {
    /// The Mesh's name (ref suffix; the identity).
    pub name: String,
    /// Active Link ids. Canonical order: sorted ascending; deduped.
    pub links: Vec<String>,
    /// The commit's message.
    pub message: String,
}
```

All addressing data lives in the Link blobs that `links` points at; the
Mesh commit itself stores only the pointers (in the tree) and the
message (on the commit).

**Invariant:** within a single Mesh, no two Links may share the same
unordered pair of sides (by `(path, start, end)`; `anchor_sha` is not
part of the key). Writes that would violate this error out. This lets
every command address a Link by its pair of ranges alone — no sha
suffix, no disambiguation step.

### 4.3 Computed views

```rust
/// Declaration order is best → worst; `Ord` derives a total order so
/// callers that want a one-line summary can reduce via `.max()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkStatus {
    Fresh,      // content unchanged since anchor
    Moved,      // location changed (rename / line shift), bytes identical
    Modified,   // content partially changed
    Rewritten,  // majority of the range rewritten
    Missing,    // range no longer locatable
    Orphaned,   // anchor commit no longer reachable
}

#[derive(Clone, Debug)]
pub struct LinkLocation {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub blob: String,
}

#[derive(Clone, Debug)]
pub struct SideResolved {
    pub anchored: LinkLocation,
    pub current: Option<LinkLocation>,
    pub status: LinkStatus,
}

#[derive(Clone, Debug)]
pub struct LinkResolved {
    pub link_id: String,
    pub anchor_sha: String,
    pub sides: [SideResolved; 2],
    /// Worse of the two side statuses.
    pub status: LinkStatus,
}

#[derive(Clone, Debug)]
pub struct MeshResolved {
    pub name: String,
    pub message: String,
    /// One resolved entry per Link id in the Mesh, in the Mesh's
    /// stored order. Each carries its own status; the Mesh does not
    /// aggregate them.
    pub links: Vec<LinkResolved>,
}
```

## 5. Resolution and staleness

### 5.1 Locate

The resolver runs once per side, independently:

```
git log -L <start>,<end>:<path> [--follow -M -C...] <anchor_sha>..HEAD
```

`git log -L` is git's line-range history walker. It performs the diff-hunk
arithmetic, handles renames when `--follow` / `-M` are on, and detects
copies per the side's `copy_detection` setting. Output is the current
`(path, start, end)` plus the blob OID of the file at HEAD for that side.

If the walker reports the range as deleted at some commit with no
surviving successor under the configured copy detection, that side's
status is `Missing`.

### 5.2 Compare

Per side, anchored bytes and current bytes are extracted from their
blobs:

```
git cat-file -p <blob> | sed -n '<start>,<end>p'
```

Equality (byte-for-byte, or normalized if `ignore_whitespace` is true)
determines that side's status. The Link's status is the worse of the
two side statuses.

Rich status — which lines changed, which commit rewrote them — is
produced by the reporter running `git blame -w -C` over the resolved
range; blame options there are internal to the reporter and not part
of the Link's stored shape.

### 5.3 Status values

| Status | Meaning |
|---|---|
| `FRESH` | Current bytes equal anchored bytes. |
| `MOVED` | Bytes equal; `(path, start, end)` changed. |
| `MODIFIED` | Some lines in the range were rewritten; most survive. |
| `REWRITTEN` | Most lines in the range were rewritten. |
| `MISSING` | No surviving location under the configured copy detection. |
| `ORPHANED` | `anchorSha` is not reachable from any ref. |

The MODIFIED / REWRITTEN threshold is a tool-level policy (for example,
"majority rewritten" = more than half the lines changed) and not stored
per Link.

### 5.4 Mesh reporting

A Mesh does not have a single status. `status`/`show` emit the per-Link
status for each Link in the Mesh; the declaration order of `LinkStatus`
(`Fresh` < `Moved` < `Modified` < `Rewritten` < `Missing` < `Orphaned`)
is provided purely as a convention so callers that need a summary can
apply their own "worst wins" reduction.

## 6. Operations

Examples use [`gix`](https://docs.rs/gix/latest/gix/) (gitoxide) rather
than shelling out to `git`. A `&gix::Repository` obtained from
`gix::open(".")` or `gix::discover(".")` is threaded through the write
functions. Errors are surfaced via `anyhow::Result` for brevity;
production code should use dedicated error types.

All writes are atomic. Link writes are a blob write + reference
update. Mesh writes are a blob write (the `links` file), a tree write,
a commit write (whose message is the Mesh's message) with the prior
Mesh tip as parent, and a reference update with an expected previous
value (compare-and-swap). If the CAS fails because another client
advanced the Mesh concurrently, the caller retries with the new tip
as parent.

### 6.1 Create a Link

```rust
use anyhow::{anyhow, Result};
use chrono::Utc;
use gix::refs::transaction::PreviousValue;
use uuid::Uuid;

pub struct CreateLinkInput {
    pub sides: [SideSpec; 2],
    pub anchor_sha: Option<String>,      // default: HEAD; shared by both sides
    pub id: Option<String>,              // default: Uuid::new_v4()
}

pub struct SideSpec {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub copy_detection: Option<CopyDetection>,
    pub ignore_whitespace: Option<bool>,
}

pub fn create_link(repo: &gix::Repository, input: CreateLinkInput)
    -> Result<(String, Link)>
{
    let anchor_sha = repo
        .rev_parse_single(input.anchor_sha.as_deref().unwrap_or("HEAD"))?
        .detach();

    let [a, b] = input.sides;
    let side_a = build_side(repo, a, &anchor_sha)?;
    let side_b = build_side(repo, b, &anchor_sha)?;
    let mut pair = [side_a, side_b];
    pair.sort();   // canonicalize so the pair is effectively unordered

    let link = Link {
        anchor_sha: anchor_sha.to_string(),
        created_at: Utc::now().to_rfc3339(),
        sides: pair,
    };

    let id       = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let text     = serialize_link(&link);
    let blob_oid = repo.write_blob(text.as_bytes())?.detach();

    // Create the ref; fail if it already exists (ids are UUIDs, collisions
    // would only arise from retried writes on content-identical blobs).
    repo.reference(
        format!("refs/links/v1/{id}"),
        blob_oid,
        PreviousValue::MustNotExist,
        format!("create link {id}"),
    )?;
    Ok((id, link))
}

/// Emit the on-disk commit-object-style format described in §4.1.
fn serialize_link(link: &Link) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "anchor {}", link.anchor_sha).unwrap();
    writeln!(out, "created {}", link.created_at).unwrap();
    for s in &link.sides {
        writeln!(out, "side {} {} {} {} {}\t{}",
                 s.start, s.end, s.blob,
                 copy_detection_str(s.copy_detection), s.ignore_whitespace,
                 s.path).unwrap();
    }
    out
}

fn build_side(
    repo: &gix::Repository,
    spec: SideSpec,
    anchor_sha: &gix::ObjectId,
) -> Result<LinkSide> {
    let blob = resolve_blob(repo, anchor_sha, &spec.path, spec.start, spec.end)?;
    Ok(LinkSide {
        path: spec.path,
        start: spec.start,
        end: spec.end,
        blob: blob.to_string(),
        copy_detection: spec.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
        ignore_whitespace: spec.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
    })
}

/// Resolve the blob OID of `path` at `anchor_sha`, after validating the
/// requested line range fits within the file.
fn resolve_blob(
    repo: &gix::Repository,
    anchor_sha: &gix::ObjectId,
    path: &str,
    start: u32,
    end: u32,
) -> Result<gix::ObjectId> {
    let commit = repo.find_commit(*anchor_sha)?;
    let tree   = commit.tree()?;
    let entry  = tree
        .lookup_entry_by_path(path)?
        .ok_or_else(|| anyhow!("{path} not found at {anchor_sha}"))?;
    let blob   = repo.find_blob(entry.id())?;
    let lines  = blob.data.iter().filter(|&&b| b == b'\n').count() as u32 + 1;
    if start < 1 || end < start || end > lines {
        return Err(anyhow!("range {start}-{end} out of bounds for {path} ({lines} lines)"));
    }
    Ok(entry.id().detach())
}
```

### 6.2 Commit a change to a Mesh

All writes — create, add, remove, reconcile, reword — are one function.
The CLI presents them as `git mesh commit <name>` with `--link`,
`--unlink`, `-m`, and `--amend`. Multiple `adds` and `removes` are
allowed in one call and land in a single git commit.

```rust
use anyhow::{anyhow, Result};
use gix::refs::transaction::PreviousValue;

pub struct CommitInput {
    pub name: String,
    /// Links to add. Each becomes a new two-sided Link.
    pub adds: Vec<[SideSpec; 2]>,
    /// Links to remove, by their pair of anchored ranges.
    pub removes: Vec<[RangeSpec; 2]>,
    /// Commit message. Also becomes the Mesh's message going forward.
    pub message: String,
    /// Shared anchor for all adds. Default: HEAD.
    pub anchor_sha: Option<String>,
    /// Reword the tip commit instead of appending a new one. Requires
    /// `adds` and `removes` to be empty.
    pub amend: bool,
}

pub fn commit_mesh(repo: &gix::Repository, input: CommitInput) -> Result<()> {
    let ref_name = format!("refs/meshes/v1/{}", input.name);
    let parent   = repo.try_find_reference(&ref_name)?
        .map(|mut r| r.peel_to_id_in_place()).transpose()?
        .map(|id| id.detach());

    // On --amend, no structural change is allowed.
    if input.amend && (!input.adds.is_empty() || !input.removes.is_empty()) {
        return Err(anyhow!("--amend is incompatible with --link/--unlink"));
    }
    // On a fresh mesh (no parent) with no adds, there's nothing to create.
    if parent.is_none() && input.adds.is_empty() {
        return Err(anyhow!("mesh `{}` does not exist; supply --link to create it",
                           input.name));
    }

    let mut links = match parent {
        Some(ref p) => read_mesh_links(repo, p)?,
        None        => Vec::new(),
    };

    // Removes first, so a reconcile ({unlink A, link A'}) can land.
    for pair in &input.removes {
        let id = find_link_by_pair(repo, &links, pair)?;
        links.retain(|l| l != &id);
    }

    let anchor_sha = repo
        .rev_parse_single(input.anchor_sha.as_deref().unwrap_or("HEAD"))?
        .detach();
    for sides in input.adds {
        let (id, _) = create_link(repo, CreateLinkInput {
            sides,
            anchor_sha: Some(anchor_sha.to_string()),
            id: None,
        })?;
        ensure_pair_unique(repo, &links, &id)?;   // enforce Mesh invariant
        links.push(id);
    }

    write_mesh(
        repo,
        &input.name,
        &sort_and_dedupe(links),
        &normalize_message(&input.message),
        parent,
        input.amend,
    )
}
```

The `write_mesh` helper builds the tree (a single `links` file with one
id per line), writes a commit using the message, and atomically updates
`refs/meshes/v1/<name>` via CAS:

```rust
use gix::objs::{tree, Commit, Tree};

fn write_mesh(
    repo: &gix::Repository,
    name: &str,
    links: &[String],
    message: &str,
    expected_parent: Option<gix::ObjectId>,
    amend: bool,
) -> Result<()> {
    // links file: one id per line, sorted/deduped, trailing newline.
    let mut file = String::new();
    for id in links {
        file.push_str(id);
        file.push('\n');
    }
    let links_blob = repo.write_blob(file.as_bytes())?.detach();

    // tree: a single entry `links` pointing at the blob.
    let tree_obj = Tree {
        entries: vec![tree::Entry {
            mode: tree::EntryKind::Blob.into(),
            filename: "links".into(),
            oid: links_blob,
        }],
    };
    let tree_id = repo.write_object(&tree_obj)?.detach();

    // --amend reuses the tip's parent; otherwise the tip itself is the parent.
    let new_parents: Vec<gix::ObjectId> = match (amend, expected_parent) {
        (true, Some(tip)) => repo.find_commit(tip)?.parent_ids()
            .map(|id| id.detach())
            .collect(),
        (true, None) => Vec::new(),
        (false, Some(tip)) => vec![tip],
        (false, None) => Vec::new(),
    };

    let (author, committer) = (repo.author()??, repo.committer()??);
    let commit = Commit {
        tree: tree_id,
        parents: new_parents.into(),
        author: author.into(),
        committer: committer.into(),
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    let commit_id = repo.write_object(&commit)?.detach();

    let ref_name = format!("refs/meshes/v1/{name}");
    let previous = match expected_parent {
        Some(p) => PreviousValue::MustExistAndMatch(p.into()),
        None    => PreviousValue::MustNotExist,
    };
    repo.reference(ref_name, commit_id, previous, format!("mesh commit"))?;
    Ok(())
}

/// Read a Mesh commit's `links` file into an ordered list of Link ids.
fn read_mesh_links(repo: &gix::Repository, commit_id: &gix::ObjectId)
    -> Result<Vec<String>>
{
    let commit = repo.find_commit(*commit_id)?;
    let tree   = commit.tree()?;
    let entry  = tree
        .lookup_entry_by_path("links")?
        .ok_or_else(|| anyhow!("mesh commit {commit_id} has no `links` file"))?;
    let blob   = repo.find_blob(entry.id())?;
    let text   = std::str::from_utf8(&blob.data)?;
    Ok(text.lines().map(str::to_owned).collect())
}

/// Load each candidate Link blob; return the id whose unordered pair
/// of side ranges equals `pair`. The Mesh invariant guarantees at most
/// one match.
fn find_link_by_pair(
    repo: &gix::Repository,
    link_ids: &[String],
    pair: &[RangeSpec; 2],
) -> Result<String> {
    let needle = canonical_pair(pair);
    for id in link_ids {
        let link = read_link(repo, id)?;
        let have = canonical_pair(&[
            RangeSpec { path: link.sides[0].path.clone(),
                        start: link.sides[0].start, end: link.sides[0].end },
            RangeSpec { path: link.sides[1].path.clone(),
                        start: link.sides[1].start, end: link.sides[1].end },
        ]);
        if have == needle {
            return Ok(id.clone());
        }
    }
    Err(anyhow!("no Link matching {}:{}",
                format_range(&pair[0]), format_range(&pair[1])))
}

/// Resolve `refs/links/v1/<id>` to its blob and parse it.
fn read_link(repo: &gix::Repository, id: &str) -> Result<Link> {
    let mut r = repo.find_reference(&format!("refs/links/v1/{id}"))?;
    let oid   = r.peel_to_id_in_place()?.detach();
    let blob  = repo.find_blob(oid)?;
    parse_link(std::str::from_utf8(&blob.data)?)
}

fn canonical_pair(pair: &[RangeSpec; 2]) -> [RangeSpec; 2] {
    let mut p = pair.clone();
    p.sort();
    p
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RangeSpec {
    pub path: String,
    pub start: u32,
    pub end: u32,
}
```

**Errors and atomicity.** `commit_mesh` validates the entire intended
change before writing any objects; a single invalid flag aborts the
call with no side effects.

- **`--link <pair>` where the Mesh already contains that pair** —
  error. The `--unlink <pair> --link <pair>` idiom is the only path
  that adds a Link with a currently-present pair, and works because
  removes are applied before adds (so the pair is absent at the
  moment the add is validated). Re-anchoring a still-valid pair at
  HEAD is this idiom's intended use.
- **`--unlink <pair>` where the Mesh does not contain that pair** —
  error with the same message shape.
- **`--amend` combined with `--link` or `--unlink`** — error;
  `--amend` is reword-only.
- **Empty invocation** (no `--link`, `--unlink`, `--amend`) — error.

Validation ordering (pure, no writes): verify the name isn't reserved;
verify `--amend` preconditions; load the current `links`; for each
`--unlink`, confirm the pair exists; for each `--link`, confirm the
pair does not collide with the post-remove set. If any check fails,
return with a specific error naming the offending pair and (for
collisions) pointing at the re-anchor idiom. Only after validation
passes does the function call `hash-object`, `mktree`, `commit-tree`,
and `update-ref`.

**Composing the common workflows** on top of `commit_mesh`:

| CLI shape                                                | `adds`    | `removes` | `amend` |
|----------------------------------------------------------|-----------|-----------|---------|
| `git mesh commit NAME --link A:B -m "..."` (fresh)       | `[A:B]`   | `[]`      | `false` |
| `git mesh commit NAME --link C:D -m "..."`               | `[C:D]`   | `[]`      | `false` |
| `git mesh commit NAME --unlink A:B -m "..."`             | `[]`      | `[A:B]`   | `false` |
| `git mesh commit NAME --unlink A:B --link A':B' -m "..."`| `[A':B']` | `[A:B]`   | `false` |
| `git mesh commit NAME --amend -m "..."`                  | `[]`      | `[]`      | `true`  |

### 6.3 Show and stale

Two read operations with different costs:

- **`show(name)`** — read-only and fast. Loads `refs/meshes/v1/<name>`,
  the `links` blob from the commit's tree, and each referenced Link
  blob. Returns the Mesh's stored state as-is: commit metadata,
  message, and per-Link `(anchor_sha, sides)` tuples. No resolver, no
  `git log -L`, no byte comparison. This is what `git mesh <name>`
  invokes.
- **`stale(name)`** — runs the resolver for every Link and produces a
  `MeshResolved` with per-Link, per-side status. Exposed as
  `git mesh stale <name>`. Computationally heavier; users invoke it
  when they want the drift picture, not just the list.

Neither has side effects; the stored Mesh is never modified.

### 6.4 History and revert

- **History:** `git log refs/meshes/v1/<name>` walks every prior state
  of the Mesh. `git log -p` shows the `links` diff per edit; each
  commit's message is the Mesh's message at that point.
- **Revert:** `git update-ref refs/meshes/v1/<name> <older-commit>`
  rolls the Mesh back to a prior state. The rolled-over commits remain
  in the object database and are reachable via `git reflog`.

### 6.5 Structural operations

These mirror git's own file-level commands (`git rm`, `git mv`,
`git restore`) and are surfaced as subcommands rather than as flags on
the name form — consistent with git's convention for destructive or
ref-shape-changing operations.

- **`git mesh rm <name>`** — delete the Mesh's ref:
  `git update-ref -d refs/meshes/v1/<name>`. Reachable commits stay in
  the object database until `git gc` collects them; the ref is gone
  immediately.
- **`git mesh mv <old> <new>`** — rename:
  `git update-ref refs/meshes/v1/<new> <commit>` followed by
  `git update-ref -d refs/meshes/v1/<old>`, both atomic. Analogous to
  `git branch -m`. If you want an alias, leave both refs in place —
  the tool exposes `git mesh mv --keep <old> <new>` as a convenience
  that omits the delete step.
- **`git mesh restore <name> <commit-ish>`** — roll the Mesh forward
  to the state at `<commit-ish>` by writing a new commit whose tree
  matches it. History is never rewritten; the restoration is a normal
  fast-forward. Equivalent in intent to `git restore --source=<rev>`
  for files.
- **Delete a Link blob** is never done directly; Links are referenced
  by Mesh commits and tracked by git's reachability. Once no Mesh
  references a Link, `git gc` collects its blob. If a Mesh somehow
  references a missing Link id (e.g. a partial clone), the resolver
  reports that Link as `ORPHANED`.

## 7. Sync

### 7.1 Refspec

```ini
[remote "origin"]
    fetch = +refs/links/*:refs/links/*
    push  = +refs/links/*:refs/links/*
    fetch = +refs/meshes/*:refs/meshes/*
    push  = +refs/meshes/*:refs/meshes/*
```

The `*` matches every schema version. To pin a client to a single version,
narrow the refspec to `refs/links/v1/*` and `refs/meshes/v1/*`.

Refspecs are configured **lazily**: the first `fetch` or `push` that
touches a remote adds missing refspec lines idempotently via
`git config --add`. There is no separate `init` step.

### 7.2 Remote visibility

Most hosts (GitHub, GitLab, Bitbucket) accept arbitrary `refs/*` namespaces
over the normal git protocol, but their web UIs do not render them. Use
`git ls-remote origin 'refs/meshes/*'` to list them. Branch-protection
rules do not apply to custom refs; the tool's write path is the integrity
boundary.

## 8. Merge semantics

### 8.1 Divergence

Two clients both edit the same Mesh on different local branches. Each
produces a new Mesh commit whose parent is the shared tip. First push
wins; the second push to `refs/meshes/v1/<id>` is rejected as
non-fast-forward, identical to a branch.

### 8.2 Three-way merge

`git merge` (or `git merge-tree` for headless resolution) performs the
standard three-way merge on the Mesh commits. Because the `links` file
is canonicalized — sorted, deduplicated, one id per line, trailing
newline — independent edits produce non-conflicting diffs. The
message lives on the commit object, so message edits are a normal
commit-message merge. Conflicts arise exactly when two branches
disagree on the same piece of state:

- Two branches reconciled the same side of the same Link to different
  new ranges (producing different replacement Link ids).
- Two branches edited the message to different text.
- Two branches added distinct new Links (usually clean; the merge
  interleaves them into the sorted list).

### 8.3 Resolving

The merge commit's tree (its `links` file) and message record the
resolution. Downstream readers see the same state. Since the prior
commits remain in history, no data is lost; a reverted decision can be
reinstated by checking out the
earlier commit and re-applying.

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

Link refs point at blobs, not commits, so no `--log`-family command
traverses them regardless of flags.

## 10. CLI reference

### 10.1 Synopsis

```
git mesh                                # list all meshes
git mesh <name>                         # show the named mesh (always read)
git mesh <subcommand> [<args>]          # everything else
```

### 10.2 Commands

Reads are the bare `<name>` form. Writes go through `git mesh commit`
(analogous to `git commit`: one invocation = one commit, create or
update). Destructive and structural operations are their own verbs,
mirroring `git rm` / `git mv` / `git restore`.

```
Reading
  git mesh                              # list every mesh (like `git branch`)
  git mesh <name>                       # show the mesh (like `git show`)
  git mesh <name> --oneline             # one line per Link, no commit header
  git mesh <name> --format=<fmt>        # format-string override
  git mesh <name> --no-abbrev           # full 40-char shas
  git mesh <name> --at <commit-ish>     # show state at a past revision
  git mesh <name> --log [--oneline] [--limit <n>]
  git mesh <name> --diff <rev>..<rev>   # compare two states
  git mesh stale <name>                 # run the resolver, report drift

Writing (one invocation = one commit; creates the mesh on first write)
  git mesh commit <name>
    [--link <rangeA>:<rangeB>] ...      # add a Link
    [--unlink <rangeA>:<rangeB>] ...    # remove a Link
    [--copy-detection off|same-commit|any-file-in-commit|any-file-in-repo]
    [--ignore-whitespace / --no-ignore-whitespace]
    [--at <commit-ish>]                 # default: HEAD; applies to new Links
    [--amend]                           # reword the tip commit instead
    (-m <msg> | -F <file> | --edit)

Structural
  git mesh rm <name>                    # delete the mesh's ref
  git mesh mv <old> <new>               # rename
  git mesh restore <name> <commit-ish>  # fast-forward to a past state
                                        # (new commit whose tree matches
                                        #  <commit-ish>; no history rewrite)

Maintenance
  git mesh fetch [<remote>]
  git mesh push  [<remote>]             # auto-configures refspec on first run
  git mesh doctor
```

**Semantics of a single `commit` invocation:**

- *Create:* `git mesh commit <name> --link <pair> -m "..."` on a name
  that doesn't yet exist. Any combination of `--link` / `--unlink` is
  accepted; the first commit is the one that creates the ref.
- *Add/remove scope:* `--link` and `--unlink` are equal citizens. Use
  either or both in one invocation.
- *Reconcile drift:* one `--unlink` paired with one `--link` in the
  same invocation — the old Link goes, the new one arrives, one commit.
- *Re-anchor:* `--unlink X:Y --link X:Y` (same pair on both flags)
  removes the existing Link and adds a fresh one at HEAD with the
  same ranges. This is the only legitimate way to add a pair the
  Mesh already contains.
- *Reword:* `--amend -m "..."` with no `--link`/`--unlink`.
- *Empty commits are an error:* `git mesh commit <name>` with no
  `--link`, `--unlink`, or `--amend` does nothing.
- *Collision errors are atomic:* `--link <pair>` on a pair the Mesh
  already contains errors before any object is written. Same for
  `--unlink <pair>` when the pair isn't present. All-or-nothing.

**Reserved names.** Mesh names cannot collide with subcommands:
`commit`, `rm`, `mv`, `restore`, `stale`, `fetch`, `push`, `doctor`,
`log`, `help`. Using any of these as a `<name>` errors at create time.

### 10.3 Range and Link syntax

```
Range      <path>#L<start>-L<end>
           e.g. src/Button.tsx#L42-L50

Link pair  <rangeA>:<rangeB>
           e.g. src/Button.tsx#L42-L50:server/routes.ts#L13-L34
```

- A **range** picks one anchored line range. The `#L<start>-L<end>`
  form matches GitHub's URL-fragment convention, so a range pasted
  from a browser URL works verbatim.
- A **Link pair** is two ranges joined by `:`. Used with `--link` and
  `--unlink` — these are the only forms that name a whole Link.

Within a Mesh, every Link has a unique unordered pair of sides (by
range), so a pair always identifies exactly one Link. There is no
single-range form for addressing a Link; a removal is always expressed
as `--unlink <rangeA>:<rangeB>`.

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

Links (<N>):
    <short-sha>  <rangeA>:<rangeB>
    <short-sha>  <rangeA>:<rangeB>
    ...
```

- **Header line** is `mesh <name>`, mirroring `git show`'s `commit <sha>`.
- **`commit`, `Author`, `Date`** are taken from the Mesh's tip commit
  object. Full sha by default for `commit`; abbreviated for Link shas.
- **Message** is indented four spaces, the same rendering `git show`
  uses. Subject, blank line, body, trailers — all as written.
- **Links section** is `<anchor-sha> <rangeA>:<rangeB>`, one per line.
  The `<rangeA>:<rangeB>` portion is exactly the syntax accepted by
  `--unlink`, so a line of output pastes directly into an edit. Order
  is the Mesh's canonical (sorted) order; stable across runs.
- `--oneline` drops the header and prints only the Links section body
  (one Link per line). `--format=<fmt>` takes a git-log-style format
  string. `--no-abbrev` shows full 40-char shas.

#### `git mesh stale`

`git mesh stale` is designed to be the daily-use command and
CI-friendly from day one. It resolves each Link in a Mesh, reports
drift per side with culprit-commit attribution, and emits a
ready-to-paste reconcile command under every stale finding.

**Staleness rule.** Any status other than `FRESH` is stale. `MOVED`
counts — if a range shifted, a human should at least glance at it
even though the bytes are identical. No gradation flag; one rule.

**Synopsis.**

```
git mesh stale [<name>]
    [--format=human|porcelain|json|junit|github-actions]
    [--exit-code]                   # non-zero if any Link is not FRESH
    [--oneline | --stat | --patch]  # detail level
    [--since <commit-ish>]          # only Links anchored at or after this commit
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

    ABC-123: front-end components reflect these back-end endpoints

2 stale of 3 links:

  MODIFIED   63d239a4  src/Button.tsx#L42-L50:server/routes.ts#L13-L34
             ├─ FRESH     src/Button.tsx#L42-L50
             └─ MODIFIED  server/routes.ts#L13-L34 → L15-L36  (3/22 lines rewritten)
                caused by 4e8b2c1a refactor: extract session helper  (2 days ago)

             reconcile with:
               git mesh commit frontend-backend-sync \
                   --unlink src/Button.tsx#L42-L50:server/routes.ts#L13-L34 \
                   --link   src/Button.tsx#L42-L50:server/routes.ts#L15-L36 \
                   -m "..."

  MOVED      a1b2c3d4  src/types.ts#L8-L15:server/schema.ts#L20-L27
             └─ MOVED     server/schema.ts#L20-L27 → L24-L31  (file unchanged, lines shifted)

  FRESH      9e8f7d6c  src/Form.tsx#L12-L40:server/validate.ts#L5-L22
```

**Output conventions.**

- **Summary line first** (`N stale of M links`), mirroring `git status`'s
  header-then-body cadence.
- **Grouped by status, worst-first:** `ORPHANED` → `MISSING` →
  `REWRITTEN` → `MODIFIED` → `MOVED` → `FRESH`.
- **Per-side tree** (`├─` / `└─`) so a one-sided drift is visually
  distinct from a two-sided one.
- **Culprit commit attribution** on `MODIFIED` / `REWRITTEN` sides: the
  short sha and subject of the commit that introduced the drift, found
  via `git log -L`. Not shown on `MOVED` — there's no meaningful culprit
  for a pure location shift.
- **Ready-to-paste reconcile command** under every stale Link. Reading
  `stale` always lands on an actionable next step.
- **Pasteable `<rangeA>:<rangeB>`** — the exact syntax `--unlink`
  accepts, so any line is copyable without edit.

**Exit codes.**

- `0` — no stale Links in scope, or `--exit-code` not passed.
- `1` — at least one Link is not `FRESH`; only returned when
  `--exit-code` is passed.
- `2` — tool error (missing repo, corrupt ref, etc.). Distinct from `1`
  so CI can tell "should fail the build" from "tool is broken."

**Machine-readable formats.**

- **`--format=porcelain`** — stable one-line-per-finding schema for
  shell pipelines.
- **`--format=json`** — versioned from v1: `{"version": 1, "mesh",
  "commit", "links": [...]}`. Each Link entry mirrors the LSP
  `Diagnostic` shape (`severity`, `range`, `message`, `code`,
  `data.culprit`, `data.reconcile_command`), so editor plugins consume
  it directly and surface squiggles + quick-fixes.
- **`--format=junit`** — generic CI integration.
- **`--format=github-actions`** — `::warning file=<path>,line=<n>::<msg>`
  annotations that land on the offending line in PR reviews.

**Build order (phased delivery).**

1. **Exit-code discipline + `--format=porcelain` and `--format=json`.**
   Without these, no automation can use the command. Ship with v1.
2. **Per-side breakdown + culprit attribution.** Turns "something is
   wrong" into "here's what and here's why."
3. **Ready-to-paste reconcile command** under each stale finding.
   Closes the feedback loop; the user never reconstructs flags by hand.
4. **`--format=github-actions`, `--format=junit`, `--since`.** Additive
   CI ergonomics once the core is solid.

Status tokens are the `LinkStatus` enum values from §4.3 (`FRESH`,
`MOVED`, `MODIFIED`, `REWRITTEN`, `MISSING`, `ORPHANED`).

### 10.5 Config

The tool reads these keys from `git config`:

```
mesh.defaultRemote  string, default "origin". Used by `git mesh fetch`
                    and `git mesh push` when no remote is supplied.
```

**Reads never touch the network.** `git mesh <name>`, `git mesh stale`,
`git mesh --log`, and every other read operation work entirely against
local object storage. To incorporate remote updates, run
`git mesh fetch` explicitly — the same discipline git applies to
`git log`, `git show`, `git status`, and `git blame`.

### 10.6 Examples

```
# Create: `git mesh commit` on a fresh name creates the ref. All Links
# share the same anchor commit (HEAD by default).
$ git mesh commit frontend-backend-sync \
      --link src/Button.tsx#L42-L50:server/routes.ts#L13-L34 \
      --link src/types.ts#L8-L15:server/schema.ts#L20-L27 \
      -m "ABC-123: front-end components reflect these back-end endpoints

Owner: team-billing"
created refs/meshes/v1/frontend-backend-sync

# Show: fast read, no resolver. Header + message + Link list with anchor shas.
$ git mesh frontend-backend-sync
mesh frontend-backend-sync
commit e0f92a3b8c1d0f5e6a7b2c4d9e1f0a3b8c1d0f5e
Author: John Wehr <john@example.com>
Date:   Wed Apr 22 11:14:03 2026 +0000

    ABC-123: front-end components reflect these back-end endpoints

    Owner: team-billing

Links (2):
    63d239a4  src/Button.tsx#L42-L50:server/routes.ts#L13-L34
    63d239a4  src/types.ts#L8-L15:server/schema.ts#L20-L27

# Stale: runs the resolver for every Link. Slower; shows drift.
$ git mesh stale frontend-backend-sync
mesh frontend-backend-sync
commit e0f92a3b...
...

Links (2):
    MODIFIED   63d239a4  src/Button.tsx#L42-L50:server/routes.ts#L13-L34
        FRESH      src/Button.tsx#L42-L50
        MODIFIED   server/routes.ts#L13-L34 → L15-L36  (3/22 lines rewritten)
    FRESH      63d239a4  src/types.ts#L8-L15:server/schema.ts#L20-L27

# Reconcile a drifted Link = --unlink the old pair, --link the new pair,
# one commit. The unchanged side is repeated verbatim.
$ git mesh commit frontend-backend-sync \
      --unlink src/Button.tsx#L42-L50:server/routes.ts#L13-L34 \
      --link   src/Button.tsx#L42-L50:server/routes.ts#L15-L36 \
      -m "routes.ts refactor"

$ git mesh stale frontend-backend-sync
...
Links (2):
    FRESH  a1b2c3d4  src/Button.tsx#L42-L50:server/routes.ts#L15-L36
    FRESH  63d239a4  src/types.ts#L8-L15:server/schema.ts#L20-L27

# Grow scope = another --link.
$ git mesh commit frontend-backend-sync \
      --link src/Form.tsx#L12-L40:server/validate.ts#L5-L22 \
      -m "track form validation too"

# Reword the tip commit (does not change links).
$ git mesh commit frontend-backend-sync --amend -m "ABC-123: front/back sync (rev 2)"

# Publish. Refspec is configured automatically on first push.
$ git mesh push
configuring refs/{links,meshes}/* on origin... done
pushed 1 mesh, 4 links
```

## 11. Appendix: why these primitives

- **Links are blobs because they are immutable and content-addressed.**
  Two identical anchors share storage automatically. The ref is a stable
  name for a piece of immutable content; "editing" a Link means creating
  a new one.
- **Meshes are commits because they are mutable with history.** A commit
  ref (name + parent chain) is exactly the shape git uses for branches,
  and the same primitive gives edit history, per-edit author/date/message,
  and native three-way merges for concurrent edits — all without any
  custom merge logic.
- **A Mesh's name is the ref name, branch-style.** No separate
  name-to-id mapping, no UUID indirection. Renames, aliases, collision
  detection, and sync all reuse git's existing ref machinery.
- **On-disk records are commit-object-style text, not JSON.** Header
  lines for structured fields, TAB before any field that may contain
  spaces (paths). Typical Link is ~150 bytes; a Mesh's message is
  stored once as the commit message rather than duplicated into the
  tree.
- **Staleness is computed, not stored, because the repo is the only
  authoritative source of whether content has changed.** A cached
  status field would either lag the truth or require eager invalidation
  on every commit; resolving on demand costs a few cheap git plumbing
  calls per member and is always correct.
- **Versioned ref namespaces (`v1`) are used instead of a payload
  `version` field so schema recognition happens at the ref level, not
  the blob level.** Readers can enumerate and filter by shape without
  opening any object, and future shapes coexist with old ones without
  migration.
