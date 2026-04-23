use anyhow::Result;

pub(crate) const RESERVED_MESH_NAMES: &[&str] = &[
    "commit", "rm", "mv", "restore", "stale", "fetch", "push", "doctor", "log", "help",
];

pub fn validate_mesh_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !RESERVED_MESH_NAMES.contains(&name),
        "mesh name `{name}` is reserved"
    );
    Ok(())
}
