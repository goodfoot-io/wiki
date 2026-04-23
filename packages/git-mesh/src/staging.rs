//! Staging area — see §6.3, §6.4.
//!
//! Transient local state under `.git/mesh/staging/` per mesh:
//! - `<name>`           — pending operations, one per line
//! - `<name>.msg`       — staged commit message (optional)
//! - `<name>.<N>`       — full-file sidecar bytes per staged `add` line
//!
//! Operation format:
//! ```text
//! add <path>#L<start>-L<end> [<anchor-sha>]
//! remove <path>#L<start>-L<end>
//! config <key> <value>
//! ```

use crate::types::CopyDetection;
use crate::Result;

/// One staged `add` line. When `anchor` is `Some`, it was frozen at
/// stage time via `--at`; when `None`, the anchor resolves to HEAD at
/// commit time and drift is checked against the working tree (§6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedAdd {
    pub line_number: u32,
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub anchor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedRemove {
    pub path: String,
    pub start: u32,
    pub end: u32,
}

/// A staged mesh-level config mutation. Multiple entries for the same
/// key collapse via last-write-wins at commit time (§6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StagedConfig {
    CopyDetection(CopyDetection),
    IgnoreWhitespace(bool),
}

/// Parsed contents of a single mesh's staging area.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Staging {
    pub adds: Vec<StagedAdd>,
    pub removes: Vec<StagedRemove>,
    pub configs: Vec<StagedConfig>,
    pub message: Option<String>,
}

/// One drift finding for a staged add whose sidecar bytes differ from
/// the working tree file (§6.3 "Validation against the working tree").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriftFinding {
    pub path: String,
    pub start: u32,
    pub end: u32,
    /// Unified diff between staged (sidecar) bytes and working-tree bytes.
    pub diff: String,
}

/// `git mesh status <name>` view model (§6.4). Pure data; the CLI
/// layer owns rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusView {
    pub name: String,
    pub staging: Staging,
    pub drift: Vec<DriftFinding>,
}

/// Read `.git/mesh/staging/<name>` and associated sidecars. Returns
/// an empty `Staging` if no file exists. Parse errors surface as
/// `Error::ParseStaging`.
pub fn read_staging(_repo: &gix::Repository, _name: &str) -> Result<Staging> {
    todo!("staging::read_staging")
}

/// Append an `add` line to the ops file and snapshot sidecar bytes.
/// `anchor = None` => read from the working tree; `anchor = Some(sha)`
/// => read from that commit's blob (§6.3).
pub fn append_add(
    _repo: &gix::Repository,
    _name: &str,
    _path: &str,
    _start: u32,
    _end: u32,
    _anchor: Option<&str>,
) -> Result<()> {
    todo!("staging::append_add — §6.3")
}

/// Append a `remove` line (§6.3). No sidecar.
pub fn append_remove(
    _repo: &gix::Repository,
    _name: &str,
    _path: &str,
    _start: u32,
    _end: u32,
) -> Result<()> {
    todo!("staging::append_remove")
}

/// Append a `config` line (§6.3). No sidecar.
pub fn append_config(
    _repo: &gix::Repository,
    _name: &str,
    _entry: &StagedConfig,
) -> Result<()> {
    todo!("staging::append_config")
}

/// Write `<name>.msg` verbatim, replacing any existing message (§6.3).
/// An empty `message` clears the file and is treated as an abort by
/// `git mesh message` (§10.2).
pub fn set_message(_repo: &gix::Repository, _name: &str, _message: &str) -> Result<()> {
    todo!("staging::set_message")
}

/// Delete every `.git/mesh/staging/<name>*` file (ops, `.msg`, sidecars).
/// Called on successful commit and by `git mesh restore` (§6.8).
pub fn clear_staging(_repo: &gix::Repository, _name: &str) -> Result<()> {
    todo!("staging::clear_staging")
}

/// Compare each staged `add` with `anchor = None` against its sidecar
/// bytes and the current working-tree bytes (§6.3).
///
/// Returns one finding per drifted range. Adds with an explicit anchor
/// are skipped (not meaningful — different tree).
pub fn drift_check(_repo: &gix::Repository, _name: &str) -> Result<Vec<DriftFinding>> {
    todo!("staging::drift_check — §6.3")
}

/// Assemble the `git mesh status <name>` view model (§6.4).
pub fn status_view(_repo: &gix::Repository, _name: &str) -> Result<StatusView> {
    todo!("staging::status_view — §6.4")
}
