use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub anchor_sha: String,
    pub created_at: String,
    pub sides: [LinkSide; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LinkSide {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub blob: String,
    pub copy_detection: CopyDetection,
    pub ignore_whitespace: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyDetection {
    Off,
    SameCommit,
    AnyFileInCommit,
    AnyFileInRepo,
}

pub const DEFAULT_COPY_DETECTION: CopyDetection = CopyDetection::SameCommit;
pub const DEFAULT_IGNORE_WHITESPACE: bool = true;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mesh {
    pub name: String,
    pub links: Vec<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredLink {
    pub id: String,
    pub anchor_sha: String,
    pub sides: [LinkSide; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshStored {
    pub name: String,
    pub message: String,
    pub links: Vec<StoredLink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkStatus {
    Fresh,
    Moved,
    Modified,
    Rewritten,
    Missing,
    Orphaned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkLocation {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub blob: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideResolved {
    pub anchored: LinkLocation,
    pub current: Option<LinkLocation>,
    pub status: LinkStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkResolved {
    pub link_id: String,
    pub anchor_sha: String,
    pub sides: [SideResolved; 2],
    pub status: LinkStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshResolved {
    pub name: String,
    pub message: String,
    pub links: Vec<LinkResolved>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateLinkInput {
    pub sides: [SideSpec; 2],
    pub anchor_sha: Option<String>,
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideSpec {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub copy_detection: Option<CopyDetection>,
    pub ignore_whitespace: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInput {
    pub name: String,
    pub adds: Vec<[SideSpec; 2]>,
    pub removes: Vec<[RangeSpec; 2]>,
    pub message: String,
    pub anchor_sha: Option<String>,
    pub amend: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RangeSpec {
    pub path: String,
    pub start: u32,
    pub end: u32,
}
