pub mod types;

pub use types::*;
use anyhow::Result;

pub fn create_link(_repo: &gix::Repository, _input: CreateLinkInput) -> Result<(String, Link)> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn commit_mesh(_repo: &gix::Repository, _input: CommitInput) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn remove_mesh(_repo: &gix::Repository, _name: &str) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn rename_mesh(_repo: &gix::Repository, _old_name: &str, _new_name: &str, _keep: bool) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn restore_mesh(_repo: &gix::Repository, _name: &str, _commit_ish: &str) -> Result<()> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn show_mesh(_repo: &gix::Repository, _name: &str) -> Result<Mesh> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn stale_mesh(_repo: &gix::Repository, _name: &str) -> Result<MeshResolved> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn serialize_link(_link: &Link) -> String {
    todo!()
}

pub fn parse_link(_text: &str) -> Result<Link> {
    Err(anyhow::anyhow!("Not implemented"))
}

pub fn read_mesh_links(_repo: &gix::Repository, _commit_id: &gix::ObjectId) -> Result<Vec<String>> {
    Err(anyhow::anyhow!("Not implemented"))
}
