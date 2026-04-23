//! git-mesh library crate.
//!
//! Public API is preserved at the crate root. The implementation is split
//! across the modules below:
//!
//! * [`types`] — data types.
//! * [`git`] — git process invocation helpers and ref transactions.
//! * [`link`] — link creation, parsing, and serialization.
//! * [`mesh`] — mesh read/commit/structural operations.
//! * [`stale`] — staleness resolution and reconcile hints.
//! * [`sync`] — fetch/push and remote refspec bootstrap.
//! * [`validation`] — mesh-name validation.

pub mod git;
pub mod link;
pub mod mesh;
pub mod stale;
pub mod sync;
pub mod types;
pub mod validation;

pub use git::read_git_text;
pub use link::{create_link, parse_link, read_link, serialize_link};
pub use mesh::{
    commit_mesh, is_ancestor_commit, list_mesh_names, mesh_commit_info, mesh_commit_info_at,
    mesh_log, read_mesh, read_mesh_at, read_mesh_links, remove_mesh, rename_mesh,
    resolve_commit_ish, restore_mesh, show_mesh, show_mesh_at,
};
pub use stale::stale_mesh;
pub use sync::{default_remote, fetch_mesh_refs, push_mesh_refs};
pub use types::*;
pub use validation::validate_mesh_name;
