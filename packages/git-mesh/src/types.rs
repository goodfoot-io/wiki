//! Data shapes for git-mesh.
//!
//! All types describe the v1 on-disk shape (see `docs/git-mesh.md` §4).
//! Every field is required; defaults are applied at creation time so
//! stored records fully self-describe their resolver behaviour.
//!
//! ## Error type
//!
//! This crate uses `thiserror` to define a library-level `Error` enum as
//! the public boundary for fallible operations. A CLI crate could reach
//! for `anyhow::Error` for brevity, but an enum-based error makes it
//! possible for downstream consumers (including the crate's own tests
//! and future library consumers) to match on variants without string
//! matching, which is the idiomatic Rust public-API choice.

use serde::{Deserialize, Serialize};

/// In-memory representation of the Range record stored at
/// `refs/ranges/v1/<rangeId>`. The id itself is the ref name suffix and
/// is not repeated in the blob.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Range {
    /// Commit this range was anchored to at creation.
    pub anchor_sha: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// File path at the anchor commit.
    pub path: String,
    /// 1-based, inclusive start line.
    pub start: u32,
    /// 1-based, inclusive end line.
    pub end: u32,
    /// Blob OID of `path` at `anchor_sha`.
    pub blob: String,
}

/// `-C` levels for `git log -L` copy detection. Stored in mesh config,
/// not in the range record. Serialized as the kebab-case variant name:
/// `off`, `same-commit`, `any-file-in-commit`, `any-file-in-repo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyDetection {
    Off,
    SameCommit,
    AnyFileInCommit,
    AnyFileInRepo,
}

/// Resolver options for all ranges in a mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MeshConfig {
    pub copy_detection: CopyDetection,
    pub ignore_whitespace: bool,
}

pub const DEFAULT_COPY_DETECTION: CopyDetection = CopyDetection::SameCommit;
pub const DEFAULT_IGNORE_WHITESPACE: bool = false;

/// A Mesh is a commit whose tree contains `ranges` and `config` files
/// and whose commit message is the Mesh's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mesh {
    /// The Mesh's name (ref suffix; the identity).
    pub name: String,
    /// Active Range ids. Canonical order: sorted by the referenced
    /// Range's `(path, start, end)` ascending.
    pub ranges: Vec<String>,
    /// The commit's message.
    pub message: String,
    /// Resolver options for all ranges in this mesh.
    pub config: MeshConfig,
}

/// Declaration order is best → worst; `Ord` derives a total order so
/// callers that want a one-line summary can reduce via `.max()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RangeStatus {
    /// Current bytes equal anchored bytes.
    Fresh,
    /// Bytes equal; `(path, start, end)` changed.
    Moved,
    /// Anchored bytes differ from current bytes, including complete deletion.
    Changed,
    /// `anchor_sha` is not reachable from any ref.
    Orphaned,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RangeLocation {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub blob: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeResolved {
    pub range_id: String,
    pub anchor_sha: String,
    pub anchored: RangeLocation,
    pub current: Option<RangeLocation>,
    pub status: RangeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshResolved {
    pub name: String,
    pub message: String,
    /// One resolved entry per Range id in the Mesh, in the Mesh's
    /// stored order.
    pub ranges: Vec<RangeResolved>,
}

/// Public error boundary for the `git-mesh` library.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("range not found: {0}")]
    RangeNotFound(String),

    #[error("mesh not found: {0}")]
    MeshNotFound(String),

    #[error("mesh already exists: {0}")]
    MeshAlreadyExists(String),

    #[error(
        "duplicate range location in mesh: {path}:{start}-{end}",
    )]
    DuplicateRangeLocation {
        path: String,
        start: u32,
        end: u32,
    },

    #[error("invalid range: start={start} end={end}")]
    InvalidRange { start: u32, end: u32 },

    #[error("parse error: {0}")]
    Parse(String),

    #[error("concurrent update: expected {expected}, found {found}")]
    ConcurrentUpdate { expected: String, found: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
