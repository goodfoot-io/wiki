# Git Mesh Project Status

This document tracks the high-level implementation status of [docs/git-mesh.md](/home/node/wiki/docs/git-mesh.md).

## Status

As of Wednesday, April 22, 2026, the `packages/git-mesh` crate has a materially expanded library implementation that satisfies the current integration test suite and validates cleanly in the workspace.

## Implemented Areas

- **Link creation**
  - `create_link` creates link records, applies defaults, canonicalizes sides, validates ranges against the requested anchor commit, writes the serialized blob, and updates `refs/links/v1/<id>`.
- **Mesh commits**
  - `commit_mesh` supports fresh mesh creation, append-only updates, duplicate-pair rejection, remove operations, reconcile operations, empty-commit rejection, amend-message updates, amend/link incompatibility checks, canonicalized `links` file output, and expected-tip/CAS-style ref updates.
- **Mesh reads**
  - `show_mesh` loads the mesh tip from `refs/meshes/v1/<name>`, reads the commit message, and returns the current stored link ids.
  - `read_mesh` loads the stored mesh state and returns full per-link stored data, including link ids, `anchor_sha`, and stored sides, without computing staleness.
- **Staleness computation**
  - `stale_mesh` now walks commit history from `anchor_sha` toward `HEAD`, tracks line movement through ancestry-path history, follows rename/copy metadata based on `copy_detection`, and classifies sides and links as `Fresh`, `Moved`, `Modified`, `Rewritten`, `Missing`, or `Orphaned` for the current tested scenarios.
- **Structural operations**
  - `remove_mesh` and `rename_mesh` perform the tested ref operations on `refs/meshes/v1/*`.
  - `restore_mesh` now writes a new mesh commit whose tree and message match the selected historical state, parented to the current tip, instead of simply repointing the ref.

## Validation

The repository currently validates cleanly with the `git-mesh` suite fully enabled.

- `packages/git-mesh`
  - `yarn lint`: passing
  - `yarn typecheck`: passing
  - `yarn test`: passing
- workspace root
  - `yarn validate`: passing

## Current Scope

The implementation is aligned with the current integration tests and the planned storage/ref model in [docs/git-mesh.md](/home/node/wiki/docs/git-mesh.md). It should still be treated as a pragmatic implementation of the currently tested behaviors rather than proof that every detail of the design document is fully realized.

Areas most likely to need further work if the project broadens:

- stronger parity with the full `git log -L`-style resolution semantics described in `docs/git-mesh.md`; the current resolver is history-aware and substantially closer to the design, but still uses a commit-by-commit diff-based approximation rather than a full `log -L` equivalent
- implementation of the CLI surface described in `docs/git-mesh.md`
- additional validation and error-shape tightening around ref/history edge cases
- refactoring and hardening beyond what the current tests require

## Test Layout

The `git-mesh` integration tests are now split by behavior:

- `packages/git-mesh/tests/create_link_integration.rs`
- `packages/git-mesh/tests/commit_mesh_integration.rs`
- `packages/git-mesh/tests/read_mesh_integration.rs`
- `packages/git-mesh/tests/stale_mesh_integration.rs`
- `packages/git-mesh/tests/structural_mesh_integration.rs`
- shared helpers in `packages/git-mesh/tests/support/mod.rs`
