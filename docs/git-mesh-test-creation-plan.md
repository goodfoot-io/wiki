# Git Mesh Test Creation Plan

## Commander's Intent

The goal of this plan is to establish a test-driven development (TDD) foundation for the `git-mesh` crate before writing any implementation code. By defining the public API boundaries (function signatures, data structures) and the behavioral expectations (skipped tests) upfront, we create a clear, actionable roadmap for the development team. 

While `docs/git-mesh.md` provides the primary architectural and behavioral specification, it is a living document. If implementers encounter technical contradictions, borrow-checker constraints, or usability issues with the proposed Rust types and signatures while writing the stubs and tests, they are empowered to adjust the signatures and structs. The paramount requirement is that the core semantics (immutable content-addressed `Link`s, mutable commit-backed `Mesh`es, computed staleness, and the specified Git storage model) remain intact.

### Primary Goal: Getting the Types Right

A critical objective of this upfront planning and stubbing phase is **type and borrow-checker validation**. Rust's strict type system means that API design choices have deep architectural consequences. By writing `#[ignore]`-annotated integration tests against function stubs and data structures:
1. **Compilation Proves Soundness:** The tests must compile, which proves that the ownership, borrowing, lifetimes, and trait bounds of the proposed API are structurally sound and usable from a consumer's perspective.
2. **Early Ergonomics Check:** We discover borrow-checker constraints or excessive cloning requirements *before* writing complex internal logic.
3. **Refinement:** If a type from the `docs/git-mesh.md` spec is unergonomic or impossible to use in the tests, implementers must refine the types and signatures immediately.

## Phase 1: Data Structures and Types

Create the core data structures outlined in the `git-mesh.md` specification in `packages/git-mesh/src/types.rs` (or similar logical modules). 

1. **Storage Models**: `Link`, `LinkSide`, `CopyDetection`, `Mesh`.
2. **Computed Views**: `LinkStatus`, `LinkLocation`, `SideResolved`, `LinkResolved`, `MeshResolved`.
3. **Input DTOs**: `CreateLinkInput`, `SideSpec`, `CommitInput`, `RangeSpec`.

*Note:* Ensure all structs derive necessary traits (`Clone`, `Debug`, `PartialEq`, `Eq`, `PartialOrd`, `Ord` where specified) and use standard types (e.g., `String`, `u32`, `bool`).

## Phase 2: Function Stubs

Create function stubs for all primary operations. These stubs must include correct argument types and return values, but **must not** contain actual implementation logic. Instead, they should immediately `todo!()` or return a generic `Err(anyhow::anyhow!("Not implemented"))`. 

Do not copy the implementation code provided in `docs/git-mesh.md`. 

### Core Write Operations
- `pub fn create_link(repo: &gix::Repository, input: CreateLinkInput) -> Result<(String, Link)>`
- `pub fn commit_mesh(repo: &gix::Repository, input: CommitInput) -> Result<()>`

### Structural Operations
- `pub fn remove_mesh(repo: &gix::Repository, name: &str) -> Result<()>`
- `pub fn rename_mesh(repo: &gix::Repository, old_name: &str, new_name: &str, keep: bool) -> Result<()>`
- `pub fn restore_mesh(repo: &gix::Repository, name: &str, commit_ish: &str) -> Result<()>`

### Read and Staleness Operations
- `pub fn show_mesh(repo: &gix::Repository, name: &str) -> Result<Mesh>`
- `pub fn stale_mesh(repo: &gix::Repository, name: &str) -> Result<MeshResolved>`

### Internal Serialization Helpers (Optional but recommended)
- `fn serialize_link(link: &Link) -> String`
- `fn parse_link(text: &str) -> Result<Link>`
- `fn read_mesh_links(repo: &gix::Repository, commit_id: &gix::ObjectId) -> Result<Vec<String>>`

## Phase 3: Skipped Integration Tests

Create a comprehensive suite of integration tests in `packages/git-mesh/tests/` (or within inline `#[cfg(test)]` modules). Every test must be fully implemented to set up a dummy Git repository (e.g., using `gix::init`), perform the operations, and assert the expected outcomes. 

**Crucially, every test must be annotated with `#[ignore]`** so the test suite passes immediately. As implementers fill out the stubs, they will remove the `#[ignore]` attributes one by one.

### 1. Link Creation Tests
- `#[ignore] fn test_create_link_success()`: Creates a link and verifies the blob is written correctly and the ref `refs/links/v1/<id>` exists.
- `#[ignore] fn test_create_link_out_of_bounds()`: Attempts to create a link with a line range outside the file's bounds and asserts an error is returned.
- `#[ignore] fn test_create_link_canonicalization()`: Verifies that passing sides in any order results in a correctly sorted/canonicalized on-disk representation.

### 2. Mesh Commit Tests
- `#[ignore] fn test_commit_mesh_create_fresh()`: Commits a new mesh with `--link` and asserts the commit, tree, and `refs/meshes/v1/<name>` are correct.
- `#[ignore] fn test_commit_mesh_add_link_to_existing()`: Commits a new link to an existing mesh.
- `#[ignore] fn test_commit_mesh_remove_link()`: Commits an `--unlink` operation and verifies the link is removed from the mesh tip.
- `#[ignore] fn test_commit_mesh_reconcile()`: Performs an `--unlink` and `--link` in the same operation (drift reconciliation).
- `#[ignore] fn test_commit_mesh_amend_message()`: Uses `--amend` to change the commit message without altering links.
- `#[ignore] fn test_commit_mesh_amend_with_links_fails()`: Verifies that passing `--amend` alongside `--link` or `--unlink` yields an error.
- `#[ignore] fn test_commit_mesh_add_existing_pair_fails()`: Ensures the invariant that a Mesh cannot contain duplicate logical pairs is upheld.
- `#[ignore] fn test_commit_mesh_remove_nonexistent_pair_fails()`: Ensures unlinking a pair not present in the Mesh yields an error.
- `#[ignore] fn test_commit_mesh_empty_fails()`: Ensures calling commit with no adds, removes, or amend yields an error.

### 3. Staleness Computation Tests
- `#[ignore] fn test_stale_mesh_fresh()`: Verifies a mesh where neither side has changed reports `LinkStatus::Fresh`.
- `#[ignore] fn test_stale_mesh_moved()`: Modifies a file before the anchored range (shifting lines down), verifies `LinkStatus::Moved`.
- `#[ignore] fn test_stale_mesh_modified()`: Modifies lines within the anchored range, verifies `LinkStatus::Modified`.
- `#[ignore] fn test_stale_mesh_rewritten()`: Modifies the majority of lines in the range, verifies `LinkStatus::Rewritten`.
- `#[ignore] fn test_stale_mesh_missing()`: Deletes the anchored file, verifies `LinkStatus::Missing`.

### 4. Structural Operation Tests
- `#[ignore] fn test_structural_rm()`: Verifies `remove_mesh` deletes the ref.
- `#[ignore] fn test_structural_mv()`: Verifies `rename_mesh` moves the ref (and `keep` semantics if implemented).
- `#[ignore] fn test_structural_restore()`: Verifies `restore_mesh` fast-forwards the ref tree state to a prior commit.

## Execution Order
The development team can proceed in any order, though building the Data Structures and Function Stubs first is recommended to allow the skipped tests to compile successfully against the type checker before implementation begins.
