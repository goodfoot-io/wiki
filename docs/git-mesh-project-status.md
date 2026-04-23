# Git Mesh Project Status

This report summarizes the current implementation state of `packages/git-mesh` relative to [docs/git-mesh.md](/home/node/wiki/docs/git-mesh.md) as of April 22, 2026.

## Overall Status

`packages/git-mesh` is no longer a narrow prototype. The crate now implements the core storage model described in the design document, exposes a broad CLI surface, maintains mesh history in git-native refs and commits, and has explicit integration coverage for the main read, write, stale-reporting, and structural workflows.

The project is in a late-stage parity phase rather than an early bring-up phase. The largest recently closed gap was write-path integrity: mesh and link mutations now use transactional ref updates with expected-old-value protection instead of ad hoc sequential ref writes.

This does not yet mean the design document is fully exhausted. The remaining work is concentrated in last-mile spec fidelity rather than foundational capability.

## Storage And Data Model

The current implementation matches the v1 storage layout in the design closely:

- Link data is stored under `refs/links/v1/*`.
- Mesh state is stored under `refs/meshes/v1/*`.
- Mesh state is commit-backed, with the commit message carrying the mesh message and a `links` file carrying sorted link ids.
- Link records include the shared `anchor_sha`, both anchored sides, blob ids, and per-side resolver settings.
- Read-side logic reconstructs mesh state from git objects rather than maintaining a side database or worktree files.

This means the project is already using the intended git-native persistence model rather than a temporary compatibility format.

## Implemented Capability Areas

### Link Creation

The library supports creation of immutable link records from two anchored ranges at a single commit. Current behavior includes:

- validation of line ranges against the requested anchor commit
- canonical ordering of the two sides before storage
- application of default resolver options on write
- serialization of the on-disk text format
- creation of the link ref in the v1 namespace
- failure on attempted ref overwrite for an existing caller-specified link id

Integration tests cover successful creation, anchor-specific blob resolution, range validation, canonicalization, and duplicate-ref rejection.

### Mesh Commit And Update Flows

`commit_mesh` now covers the practical mesh lifecycle expected by the spec:

- creating a new mesh
- appending links to an existing mesh
- removing links from an existing mesh
- reconcile-style remove-and-add updates
- duplicate-pair rejection
- empty non-amend commit rejection
- amend-message updates without changing the parent chain
- expected-tip enforcement for explicit compare-and-swap workflows

The links file written into mesh commits is canonicalized on write, and the implementation rejects operations that would violate the tested invariants.

### Transactional Write Hardening

The current tree includes a meaningful hardening pass on mutating operations:

- `commit_mesh` validates and prepares link blobs before publishing refs
- link ref creation and mesh tip movement are staged in a single `git update-ref --stdin` transaction
- implicit-tip mesh updates retry on transactional race instead of failing immediately on a stale observed tip
- explicit expected-tip workflows still fail closed on stale state
- failed mesh commits do not leave partially published link refs behind
- `remove_mesh`, `rename_mesh`, and `restore_mesh` now use transactional expected-old-value semantics rather than loose sequential updates

This is a material improvement in practical correctness and aligns the implementation much more closely with the design’s atomicity requirements.

### Read-Side Inspection

The package supports direct inspection of stored mesh and link state, including:

- showing the current mesh tip
- showing a mesh at an explicit historical commit-ish
- reading stored link payloads
- reading stored mesh state with fully expanded link records
- listing mesh names
- reading mesh commit metadata
- mesh history and diff-oriented inspection helpers used by the CLI

The project is therefore beyond “write-only” functionality; operators can inspect both current and historical mesh state from the CLI and library.

### Stale Resolution And Reporting

`git-mesh` now has a substantial stale-analysis surface. The current implementation includes:

- per-side and per-link resolution against current `HEAD`
- status reporting for fresh, moved, modified, rewritten, missing, and orphaned conditions in the tested scenarios
- human-readable output
- porcelain output
- JSON output
- JUnit output
- GitHub Actions output
- support for `--since`
- support for patch-oriented stale reporting
- culprit attribution support and related reconcile workflows already added in earlier slices

This area appears broadly implemented from a CLI perspective. Remaining risk here is more about semantic fidelity than missing command entry points.

### Structural Operations

The structural mesh workflows described in the spec are present in practical form:

- `rm`
- `mv`
- `restore`

The restore path writes a new mesh commit representing the restored historical state rather than simply repointing the ref, which preserves mesh history in the intended shape.

### Sync And Repository Health Tooling

The package also includes supporting operational commands:

- `fetch`
- `push`
- lazy sync refspec bootstrap
- `doctor`

Those commands move the project closer to a usable day-to-day tool rather than a library demo.

### CLI Parity Improvements Already Landed

Recent slices closed several important CLI gaps, including:

- `stale --patch`
- `show --format=<fmt>`
- `commit -F`
- `commit --edit`
- `commit --no-ignore-whitespace`

Taken together with the other subcommands already present in `src/main.rs`, the current CLI surface is broad and recognizably aligned with the design document.

## Validation Status

The current worktree has already been validated successfully for this implementation slice.

Validated commands:

- `yarn lint` in `packages/git-mesh`
- `yarn typecheck` in `packages/git-mesh`
- `yarn test` in `packages/git-mesh`
- `yarn validate` at the workspace root

All of those commands completed successfully on the current tree before this report was written. The report itself is Markdown-only and does not change runtime behavior.

## Test Coverage Shape

The integration suite is organized by behavior area and now covers the major user-visible workflows:

- link creation
- mesh commit/update behavior
- read-side inspection
- stale reporting
- structural operations
- CLI integration behavior
- shared repository helpers in test support

Recent coverage additions specifically target race handling and partial-write prevention for mesh mutations, which were previously under-specified in tests relative to their importance in the design.

## What Is Strongly Aligned With The Spec

The following design themes are now implemented in a way that appears materially faithful to `docs/git-mesh.md`:

- git-native storage under versioned custom refs
- immutable link records and mutable commit-backed mesh tips
- read-time computation of stale state instead of stored status fields
- mesh history via parent-linked commits
- broad CLI coverage across creation, inspection, stale analysis, sync, and repair workflows
- fail-closed ref update behavior for important mutations
- test-backed enforcement of several output and integrity contracts

## Remaining Gaps And Likely Next Areas Of Work

The remaining work is not dominated by missing primitives. It is more likely to be found in one of these buckets:

- output and UX details that still differ from the exact contracts in `docs/git-mesh.md`
- deeper resolver fidelity versus true `git log -L` semantics in edge cases not yet covered by tests
- merge and divergence workflows beyond the currently implemented ref-transaction and restore coverage
- any final flag or formatting mismatches that have not yet been exercised by integration tests
- possible cleanup of test-only machinery used to deterministically simulate races

The most important observation is that the open work now appears to be “spec polishing and edge-case fidelity,” not “build the main system.”

## Practical Assessment

If evaluated as a practical tool rather than as a line-by-line proof of complete spec conformance, `packages/git-mesh` is in strong shape:

- the core object model is implemented
- the main CLI workflows exist
- the write path is materially safer than it was before the latest slice
- the package validates cleanly
- the remaining uncertainty is concentrated in fidelity and completeness, not viability

That is a meaningful milestone. The project has moved from capability expansion into the narrower work of tightening parity with the design document.
