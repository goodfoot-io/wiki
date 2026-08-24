//! Pass orchestrator: `Tree -> Index -> Worktree` merged into a single
//! `seen_paths` map.
//!
//! All three passes emit [`PassDelta`] values against the newest retained
//! generation's recorded inputs (delta base, plan D5); the orchestrator
//! merges them with strict later-source-wins ordering and builds a
//! [`PublishCandidate`] entirely outside any write transaction — parsing,
//! object-database reads, and hashing all precede [`GenerationsStore::
//! publish`], whose single atomic transaction materializes the generation,
//! its `gen_paths` rows, the global blob upserts, and the per-generation
//! FTS child. Nothing is ever mutated in place: immutability replaces the
//! old CAS-on-state-row machinery.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::index::freshness;
use crate::index::generations::{
    self, GenPathRow, Generation, GenerationsStore, PublishCandidate, StateFingerprint,
    EMPTY_TREE_BASE, ZERO_INDEX_CHECKSUM,
};
use crate::index::ingest::{WikiBlobFields, parse_blob};
use crate::index::{BlobOid, HostileFs, Source};

pub mod index_file;
pub mod tree;
pub mod worktree;

/// One change observed by a single pass.
#[derive(Debug, Clone)]
pub struct PassDelta {
    /// Repo-root-relative path.
    pub path: PathBuf,
    pub source: Source,
    pub action: DeltaAction,
}

#[derive(Debug, Clone)]
pub enum DeltaAction {
    /// Path is present with the given blob OID.
    Add {
        oid: BlobOid,
        /// Bytes read during the pass, for sources where the pass
        /// reads file content itself (e.g. Worktree). When `Some`,
        /// the candidate builder uses these instead of re-reading
        /// from disk, eliminating the TOCTOU window between hash
        /// and ingest.
        blob_bytes: Option<Vec<u8>>,
        /// File mtime at the moment of hashing, stored in
        /// `gen_paths.stat_mtime_ns` so the next refresh can detect
        /// in-place content edits.
        stat_mtime_ns: Option<i64>,
    },
    /// Path is no longer present in this source.
    Remove,
    /// Pass 1 pure rename — same blob OID at a new path. `from` is the
    /// previous path; the new path is the delta's `path`. Rewrites that
    /// also change content are decomposed into `Remove` + `Add` instead,
    /// since the blob row does not carry over.
    Rename { from: PathBuf, oid: BlobOid },
}

/// Counters returned from the orchestrator.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefreshOutcome {
    pub fts_retokenizations: u64,
    pub pass3_full_rescans: u64,
    /// Number of directories Pass 3 descended into and stat/hashed
    /// markdown files for. The dir-mtime Merkle short-circuit is gone
    /// (plan D6): every directory is walked, and carry-forward happens at
    /// `(path_rel, stat_mtime_ns)` file granularity.
    pub pass3_dir_walks: u64,
    /// True when publish reported [`PublishOutcome::ConflictDiscarded`] —
    /// an identical generation already existed, so the winner (byte-for-
    /// byte the same canonical state) is served instead.
    pub conflict_discarded: bool,
    /// The generation to serve after the refresh, whatever the outcome.
    pub served_gen_id: i64,
}

/// Resolve a blob's bytes for the requested OID.
///
/// `Source::Tree` and `Source::Index` rows must read from the git object
/// database — reading the worktree file would silently substitute the
/// worktree's content for HEAD's or the index's content whenever they
/// diverge (the steady-state developer condition). `Source::Worktree`
/// rows hashed the on-disk bytes to produce the OID in the first place,
/// so reading from disk is the only option there: untracked and
/// gitignored blobs are typically absent from the ODB.
fn read_blob_bytes(
    repo: &gix::Repository,
    repo_root: &Path,
    source: Source,
    path_rel: &Path,
    oid: &BlobOid,
) -> Result<Vec<u8>> {
    match source {
        Source::Tree | Source::Index => {
            let id = gix::ObjectId::from_hex(oid.0.as_bytes())
                .map_err(|e| anyhow::anyhow!("invalid blob oid `{}`: {e}", oid.0))?;
            let blob = repo
                .find_blob(id)
                .map_err(|e| anyhow::anyhow!("blob {} not in odb: {e}", oid.0))?;
            Ok(blob.data.to_vec())
        }
        Source::Worktree => {
            let abs = repo_root.join(path_rel);
            std::fs::read(&abs).map_err(|e| anyhow::anyhow!("read worktree {}: {e}", abs.display()))
        }
    }
}

/// SHA-1 of the `.wiki/.wikiignore` contents, or the 20-zero sentinel when
/// the file is absent. Part of the canonical fingerprint: the Tree pass is
/// diff-based and cannot observe a wikiignore-only commit, so a change in
/// this hash relative to the base generation forces a full bidirectional
/// Tree reconciliation.
pub(crate) fn compute_wikiignore_hash(repo_root: &Path) -> [u8; 20] {
    let path = repo_root.join(".wiki").join(".wikiignore");
    let mut out = [0u8; 20];
    if let Ok(bytes) = std::fs::read(&path) {
        let digest = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &bytes)
            .expect("SHA-1 hashing is infallible")
            .as_bytes()
            .to_vec();
        out.copy_from_slice(&digest);
    }
    out
}

/// Drive Pass 1, Pass 2, Pass 3 against the newest generation's delta base,
/// build the publish candidate outside any transaction, and publish it
/// atomically. On conflict-discard the existing (identical) generation is
/// the serve target.
pub fn refresh(
    repo: &gix::Repository,
    repo_root: &Path,
    dot_git: &Path,
    store: &GenerationsStore,
    hostile_fs: HostileFs,
) -> Result<RefreshOutcome> {
    // Delta base: the newest retained generation, or a cold start.
    let base: Option<Generation> = store.newest()?;
    let base_rows = match &base {
        Some(generation) => store.all_generation_paths(generation.gen_id)?,
        None => Vec::new(),
    };

    let prior_head_tree = base
        .as_ref()
        .map_or(EMPTY_TREE_BASE.to_string(), |g| g.fingerprint.head_tree_oid.clone());
    let prior_head_tree_oid = if prior_head_tree.is_empty() {
        None
    } else {
        Some(
            gix::ObjectId::from_hex(prior_head_tree.as_bytes())
                .map_err(|e| anyhow::anyhow!("decode prior head_tree_oid: {e}"))?,
        )
    };
    let prior_index_checksum_arr: [u8; 20] =
        base.as_ref().map_or(ZERO_INDEX_CHECKSUM, |g| g.fingerprint.index_checksum);

    // Load WikiIgnore once; all three passes apply the same filter so
    // wikiignored paths are never ingested regardless of source.
    let wiki_ignore = crate::wikiignore::WikiIgnore::load(repo_root)?;

    // Hash the current `.wiki/.wikiignore` contents (20-zero sentinel when
    // absent). A change relative to the base generation is the sole signal
    // to run the full bidirectional Tree reconciliation (un-ignore re-adds
    // as well as ignore removes).
    let new_wikiignore_hash = compute_wikiignore_hash(repo_root);
    let wikiignore_changed = match &base {
        Some(generation) => new_wikiignore_hash != generation.fingerprint.wikiignore_hash,
        None => true,
    };

    // Member set seeded from the base generation: carried rows stay
    // byte-identical unless a delta replaces them.
    let mut members: HashMap<(Source, String), GenPathRow> = base_rows
        .into_iter()
        .map(|row| ((row.source, row.path_rel.clone()), row))
        .collect();
    // Global blob identities: an oid present in `blobs` is parsed once ever
    // (content-addressed across all generations).
    let known_oids: HashSet<String> = {
        let conn = store.conn();
        let mut stmt = conn.prepare_cached("SELECT oid FROM blobs")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()?
    };
    let base_tree_paths: Vec<String> =
        members.iter().filter(|((source, _), _)| *source == Source::Tree).map(|((_, path), _)| path.clone()).collect();
    let existing_tree_rows: HashSet<String> =
        members.iter().filter(|((source, _), _)| *source == Source::Tree).map(|((_, path), _)| path.clone()).collect();
    let prior_index_oids: HashMap<PathBuf, String> = members
        .iter()
        .filter(|((source, _), _)| *source == Source::Index)
        .map(|((_, path), member)| (PathBuf::from(path), member.oid.0.clone()))
        .collect();
    let base_worktree_rows: Vec<GenPathRow> =
        members.values().filter(|row| row.source == Source::Worktree).cloned().collect();

    let mut candidate_builder = CandidateBuilder {
        members: &mut members,
        new_blobs: Vec::new(),
        queued_new: HashSet::new(),
        known_oids,
        non_wiki_oids: HashSet::new(),
        fts_retokenizations: 0,
    };

    // Pass 1: Tree (committed snapshot).
    let mut tree_deltas = crate::perf::scope_result("index.pass_tree", serde_json::json!({}), || {
        tree::pass_tree(repo, prior_head_tree_oid, &wiki_ignore)
    })?;
    // Sweep the base Tree rows and emit Remove deltas for any path that is
    // now wikiignored but was not touched by the tree diff (e.g. the
    // wikiignore pattern landed in a commit while the file itself was
    // unchanged — the diff produces no delta for the file).
    {
        for row in base_tree_paths {
            let pb = std::path::PathBuf::from(&row);
            if wiki_ignore.is_ignored(&pb) {
                let already_removed = tree_deltas.iter().any(|d| {
                    d.path == pb && matches!(d.action, DeltaAction::Remove)
                });
                if !already_removed {
                    tree_deltas.push(PassDelta {
                        path: pb,
                        source: Source::Tree,
                        action: DeltaAction::Remove,
                    });
                }
            }
        }
    }

    // Un-ignore direction (symmetric counterpart of the sweep above). Only
    // run when the wikiignore changed since the base generation.
    if wikiignore_changed {
        for (path, oid) in tree::head_tree_markdown_entries(repo)? {
            if wiki_ignore.is_ignored(&path) {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if existing_tree_rows.contains(&path_str) {
                continue;
            }
            let already_added = tree_deltas.iter().any(|d| {
                d.path == path && matches!(d.action, DeltaAction::Add { .. } | DeltaAction::Rename { .. })
            });
            if already_added {
                continue;
            }
            tree_deltas.push(PassDelta {
                path,
                source: Source::Tree,
                action: DeltaAction::Add {
                    oid,
                    blob_bytes: None,
                    stat_mtime_ns: None,
                },
            });
        }
    }

    // Pass 2: Index file.
    let index_deltas =
        crate::perf::scope_result("index.pass_index", serde_json::json!({}), || {
            index_file::pass_index(
                dot_git,
                &prior_index_checksum_arr,
                &prior_index_oids,
                &wiki_ignore,
                wikiignore_changed,
            )
        })?;

    // Pass 3: Worktree.
    let mut pass3_full_rescans: u64 = 0;
    let mut pass3_dir_walks: u64 = 0;
    let worktree_deltas =
        crate::perf::scope_result("index.pass_worktree", serde_json::json!({}), || {
            worktree::pass_worktree(
                repo,
                repo_root,
                &base_worktree_rows,
                hostile_fs,
                &mut pass3_full_rescans,
                &mut pass3_dir_walks,
                &wiki_ignore,
            )
        })?;

    // Merge deltas in strict order Tree -> Index -> Worktree; later
    // sources win per (source, path).
    let all_deltas: Vec<PassDelta> = tree_deltas
        .into_iter()
        .chain(index_deltas)
        .chain(worktree_deltas)
        .collect();

    crate::perf::scope_result(
        "index.apply_deltas",
        serde_json::json!({ "deltas": all_deltas.len() }),
        || -> Result<()> {
            for delta in &all_deltas {
                match &delta.action {
                    DeltaAction::Add { oid, blob_bytes, stat_mtime_ns } => {
                        candidate_builder.add(
                            repo,
                            repo_root,
                            &delta.path,
                            delta.source,
                            oid,
                            blob_bytes.as_deref(),
                            *stat_mtime_ns,
                        )?;
                    }
                    DeltaAction::Remove => {
                        candidate_builder.remove(delta.source, &delta.path);
                    }
                    DeltaAction::Rename { from, oid } => {
                        // A pure rename carries an already-known wiki blob.
                        // For a non-wiki (never-ingested) oid the rename is
                        // a removal at both ends — inserting a destination
                        // row would violate the gen_paths→blobs FK, exactly
                        // like the old decompose-into-Remove+Add guard.
                        let wiki_known = candidate_builder.known_oids.contains(&oid.0)
                            || candidate_builder.queued_new.contains(&oid.0);
                        if wiki_known {
                            candidate_builder.rename(from, &delta.path, delta.source, oid);
                        } else {
                            candidate_builder.remove(delta.source, from);
                        }
                    }
                }
            }
            Ok(())
        },
    )?;

    // Canonical fingerprint of the post-refresh state — computed from the
    // same legs the gate uses (with sentinel fallbacks for unborn HEAD /
    // missing index), so the very next gate hits byte-identically.
    let fingerprint: StateFingerprint =
        freshness::published_fingerprint(repo, repo_root, dot_git, &new_wikiignore_hash);

    let mut paths: Vec<GenPathRow> = candidate_builder.members.values().cloned().collect();
    paths.sort_by(|a, b| {
        a.path_rel
            .cmp(&b.path_rel)
            .then_with(|| source_rank(a.source).cmp(&source_rank(b.source)))
    });

    let candidate = PublishCandidate {
        fingerprint,
        publisher: Some(dot_git.to_string_lossy().to_string()),
        paths,
        new_blobs: std::mem::take(&mut candidate_builder.new_blobs),
    };
    let fts_retokenizations = candidate_builder.fts_retokenizations;

    // Compute-outside-write-txn invariant: the candidate above is inert
    // data; publish owns the only transaction of the refresh.
    let outcome = crate::perf::scope_result("index.publish", serde_json::json!({}), || {
        store.publish(candidate).map_err(|e| anyhow::anyhow!("publish: {e}"))
    })?;

    Ok(match outcome {
        generations::PublishOutcome::Published { generation } => RefreshOutcome {
            fts_retokenizations,
            pass3_full_rescans,
            pass3_dir_walks,
            conflict_discarded: false,
            served_gen_id: generation.gen_id,
        },
        generations::PublishOutcome::ConflictDiscarded { existing } => RefreshOutcome {
            fts_retokenizations: 0,
            pass3_full_rescans,
            pass3_dir_walks,
            conflict_discarded: true,
            served_gen_id: existing.gen_id,
        },
    })
}

fn source_rank(source: Source) -> u8 {
    match source {
        Source::Tree => 0,
        Source::Index => 1,
        Source::Worktree => 2,
    }
}

/// Accumulates the publish candidate from merged deltas — pure in-memory
/// computation, no database access beyond the read-only `known_oids`
/// snapshot taken before any pass runs.
struct CandidateBuilder<'a> {
    /// Live member set across all three sources (seeded from the base
    /// generation, mutated by deltas).
    members: &'a mut HashMap<(Source, String), GenPathRow>,
    /// Blobs newly ingested this refresh (global upserts).
    new_blobs: Vec<(BlobOid, WikiBlobFields)>,
    /// Oids already appended to `new_blobs` this refresh.
    queued_new: HashSet<String>,
    /// Oids present in the global `blobs` table at refresh start.
    known_oids: HashSet<String>,
    /// Oids whose bytes failed `parse_blob` during this refresh.
    non_wiki_oids: HashSet<String>,
    fts_retokenizations: u64,
}

impl<'a> CandidateBuilder<'a> {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        repo: &gix::Repository,
        repo_root: &Path,
        path_rel: &Path,
        source: Source,
        oid: &BlobOid,
        blob_bytes: Option<&[u8]>,
        stat_mtime_ns: Option<i64>,
    ) -> Result<()> {
        let path_str = path_rel.to_string_lossy().to_string();
        // Non-wiki blobs never gain a member row; a stale carried row at
        // this (path, source) is dropped.
        if self.non_wiki_oids.contains(&oid.0) {
            self.members.remove(&(source, path_str));
            return Ok(());
        }
        if !self.known_oids.contains(&oid.0) && !self.queued_new.contains(&oid.0) {
            let bytes: Vec<u8> = match blob_bytes {
                Some(b) => b.to_vec(),
                None => read_blob_bytes(repo, repo_root, source, path_rel, oid)?,
            };
            let fields: Option<WikiBlobFields> = parse_blob(&bytes);
            match fields {
                Some(fields) => {
                    self.new_blobs.push((oid.clone(), fields));
                    self.queued_new.insert(oid.0.clone());
                    // Later deltas for the same oid must not re-parse or
                    // re-queue: content is immutable per oid.
                    self.known_oids.insert(oid.0.clone());
                    self.fts_retokenizations += 1;
                }
                None => {
                    self.non_wiki_oids.insert(oid.0.clone());
                    self.members.remove(&(source, path_str));
                    return Ok(());
                }
            }
        }
        let parent_dir =
            path_rel.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        self.members.insert(
            (source, path_str),
            GenPathRow {
                source,
                path_rel: path_rel.to_string_lossy().to_string(),
                oid: oid.clone(),
                parent_dir,
                stat_mtime_ns,
            },
        );
        Ok(())
    }

    fn remove(&mut self, source: Source, path_rel: &Path) {
        self.members.remove(&(source, path_rel.to_string_lossy().to_string()));
    }

    /// Pure rename: same oid moves paths within one source. A displaced
    /// destination row is dropped outright — blob release is implicit in
    /// publish's set-based refcount reconciliation, which closes the old
    /// clobbered-rename leak by construction.
    fn rename(&mut self, from: &Path, to: &Path, source: Source, oid: &BlobOid) {
        self.remove(source, from);
        self.remove(source, to);
        let parent_dir = to.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        self.members.insert(
            (source, to.to_string_lossy().to_string()),
            GenPathRow {
                source,
                path_rel: to.to_string_lossy().to_string(),
                oid: oid.clone(),
                parent_dir,
                stat_mtime_ns: None,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::blob::compute_blob_oid;
    use std::process::Command;

    /// `CandidateBuilder::add` uses the bytes carried through
    /// `DeltaAction::Add::blob_bytes` instead of re-reading from disk,
    /// eliminating the TOCTOU window between the pass-3 read and ingest:
    /// even when the file changes on disk between read and add, the
    /// candidate's new-blob fields come from the carried bytes.
    #[test]
    fn worktree_add_uses_carried_bytes() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path();

        let status = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(root)
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed");
        let repo = gix::open(root).expect("gix open");

        let rel = Path::new("page.md");
        let content_a =
            b"---\ntitle: Original\nsummary: Original content.\n---\n\nBody original.\n";
        std::fs::write(root.join(rel), content_a).expect("write A");
        let oid_a = compute_blob_oid(content_a);

        let mut members = HashMap::new();
        let mut builder = CandidateBuilder {
            members: &mut members,
            new_blobs: Vec::new(),
            queued_new: HashSet::new(),
            known_oids: HashSet::new(),
            non_wiki_oids: HashSet::new(),
            fts_retokenizations: 0,
        };

        builder
            .add(&repo, root, rel, Source::Worktree, &oid_a, Some(content_a.as_ref()), None)
            .expect("add");

        let row = builder
            .members
            .get(&(Source::Worktree, "page.md".to_string()))
            .expect("member row created");
        assert_eq!(row.oid.0, oid_a.0);

        let (new_oid, fields) =
            builder.new_blobs.first().expect("unseen blob queued as a global upsert");
        assert_eq!(new_oid.0, oid_a.0);
        assert_eq!(fields.title, "Original", "fields must come from carried bytes");
        assert_eq!(fields.summary, "Original content.");
    }

    /// A pure rename keeps the oid and drops any displaced destination row;
    /// release bookkeeping is implicit in publish's set-based refcount
    /// reconciliation.
    #[test]
    fn rename_drops_displaced_destination_row() {
        let mut members = HashMap::new();
        let mut builder = CandidateBuilder {
            members: &mut members,
            new_blobs: Vec::new(),
            queued_new: HashSet::new(),
            known_oids: HashSet::new(),
            non_wiki_oids: HashSet::new(),
            fts_retokenizations: 0,
        };

        let ghost = BlobOid("f".repeat(40));
        let real = BlobOid("a".repeat(40));
        builder.members.insert(
            (Source::Tree, "dest.md".to_string()),
            GenPathRow {
                source: Source::Tree,
                path_rel: "dest.md".into(),
                oid: ghost,
                parent_dir: String::new(),
                stat_mtime_ns: None,
            },
        );

        builder.rename(Path::new("source.md"), Path::new("dest.md"), Source::Tree, &real);

        let row = builder
            .members
            .get(&(Source::Tree, "dest.md".to_string()))
            .expect("destination row present");
        assert_eq!(row.oid, real);
        assert!(!builder.members.contains_key(&(Source::Tree, "source.md".to_string())));
        assert_eq!(builder.members.len(), 1, "displaced ghost row fully dropped");
    }
}
