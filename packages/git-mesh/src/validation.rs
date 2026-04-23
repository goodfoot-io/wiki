//! Name validation (§3.5, §10.2 reserved list).

use crate::{Error, Result};

/// Subcommands and reserved tokens that cannot be used as mesh names.
/// From §10.2 "Reserved names."
pub const RESERVED_MESH_NAMES: &[&str] = &[
    "add", "rm", "commit", "message", "restore", "revert", "delete", "mv", "stale", "fetch",
    "push", "doctor", "log", "config", "status", "ls", "help",
];

/// Validate a mesh name against §3.5 and §10.2:
/// - ref-legal path component (no slashes, whitespace, control chars,
///   no leading `-`)
/// - not on the reserved list
pub fn validate_mesh_name(name: &str) -> Result<()> {
    if RESERVED_MESH_NAMES.contains(&name) {
        return Err(Error::ReservedName(name.to_string()));
    }
    validate_ref_component(name)
}

/// Validate a range id (UUID, ref-legal). Range ids are internal; users
/// never type them, but the CLI and resolver both verify shape.
pub fn validate_range_id(id: &str) -> Result<()> {
    validate_ref_component(id)
}

fn validate_ref_component(_value: &str) -> Result<()> {
    todo!("validation::validate_ref_component — §3.5 rules")
}
