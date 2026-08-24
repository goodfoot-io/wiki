//! Pass 1 — diff `HEAD^{tree}` against the previously-recorded tree OID
//! (or the empty tree for first-run / unborn HEAD), filtered to `.md`
//! paths.

use std::path::PathBuf;

use anyhow::Result;

use crate::index::{BlobOid, Source};
use crate::wikiignore::WikiIgnore;

use super::{DeltaAction, PassDelta};

/// SHA-1 of the empty tree, used as the prior tree when `last_head_tree_oid`
/// is `None`.
const EMPTY_TREE_OID: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub fn pass_tree(
    repo: &gix::Repository,
    last_head_tree_oid: Option<gix::ObjectId>,
    wiki_ignore: &WikiIgnore,
) -> Result<Vec<PassDelta>> {
    let new_tree_id = match repo.head_tree_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok(Vec::new()),
    };

    let old_tree_id = last_head_tree_oid.unwrap_or_else(|| {
        gix::ObjectId::from_hex(EMPTY_TREE_OID.as_bytes()).expect("static empty tree oid")
    });

    if old_tree_id == new_tree_id {
        return Ok(Vec::new());
    }

    let new_tree = repo
        .find_tree(new_tree_id)
        .map_err(|e| anyhow::anyhow!("find new tree {}: {e}", new_tree_id))?;

    let old_tree_obj = repo.find_tree(old_tree_id).ok();

    let mut opts = gix::diff::Options::default();
    // Rename tracking only pays off against a real prior tree. The
    // first-run diff against the empty tree is all additions — there are
    // no deletions to pair renames with, so skip the rewrite matcher.
    if last_head_tree_oid.is_some() {
        opts.track_rewrites(Some(gix::diff::Rewrites::default()));
    }

    let changes = repo
        .diff_tree_to_tree(old_tree_obj.as_ref(), Some(&new_tree), opts)
        .map_err(|e| anyhow::anyhow!("diff_tree_to_tree: {e}"))?;

    let mut out = Vec::new();
    for change in changes {
        match change {
            gix::object::tree::diff::ChangeDetached::Addition { location, id, .. } => {
                let path = bstring_to_path(&location);
                if !is_markdown(&path) {
                    continue;
                }
                if wiki_ignore.is_ignored(&path) {
                    continue;
                }
                out.push(PassDelta {
                    path,
                    source: Source::Tree,
                    action: DeltaAction::Add {
                        oid: BlobOid(id.to_hex().to_string()),
                        blob_bytes: None,
                        stat_mtime_ns: None,
                    },
                });
            }
            gix::object::tree::diff::ChangeDetached::Deletion { location, .. } => {
                let path = bstring_to_path(&location);
                if !is_markdown(&path) {
                    continue;
                }
                out.push(PassDelta {
                    path,
                    source: Source::Tree,
                    action: DeltaAction::Remove,
                });
            }
            gix::object::tree::diff::ChangeDetached::Modification { location, id, .. } => {
                let path = bstring_to_path(&location);
                if !is_markdown(&path) {
                    continue;
                }
                if wiki_ignore.is_ignored(&path) {
                    // Emit a Remove so any previously-indexed row is cleared.
                    out.push(PassDelta {
                        path,
                        source: Source::Tree,
                        action: DeltaAction::Remove,
                    });
                    continue;
                }
                out.push(PassDelta {
                    path,
                    source: Source::Tree,
                    action: DeltaAction::Add {
                        oid: BlobOid(id.to_hex().to_string()),
                        blob_bytes: None,
                        stat_mtime_ns: None,
                    },
                });
            }
            gix::object::tree::diff::ChangeDetached::Rewrite {
                source_location,
                source_id,
                location,
                id,
                ..
            } => {
                let from = bstring_to_path(&source_location);
                let to = bstring_to_path(&location);
                match (is_markdown(&from), is_markdown(&to)) {
                    // Pure rename: the blob row and refcount carry over
                    // unchanged, so `apply_rename`'s row-level path swap
                    // is sound and avoids a retokenization.
                    (true, true) if source_id == id => {
                        if wiki_ignore.is_ignored(&to) {
                            // Renamed into an ignored path — drop the old row.
                            out.push(PassDelta {
                                path: from,
                                source: Source::Tree,
                                action: DeltaAction::Remove,
                            });
                        } else {
                            out.push(PassDelta {
                                path: to,
                                source: Source::Tree,
                                action: DeltaAction::Rename {
                                    from,
                                    oid: BlobOid(id.to_hex().to_string()),
                                },
                            });
                        }
                    }
                    // Rename + edit: the new OID needs a `blobs` row and
                    // the old one needs releasing, so decompose into
                    // Remove + Add for full blob bookkeeping.
                    (true, true) => {
                        out.push(PassDelta {
                            path: from,
                            source: Source::Tree,
                            action: DeltaAction::Remove,
                        });
                        if !wiki_ignore.is_ignored(&to) {
                            out.push(PassDelta {
                                path: to,
                                source: Source::Tree,
                                action: DeltaAction::Add {
                                    oid: BlobOid(id.to_hex().to_string()),
                                    blob_bytes: None,
                                    stat_mtime_ns: None,
                                },
                            });
                        }
                    }
                    // Renamed out of the wiki.
                    (true, false) => {
                        out.push(PassDelta {
                            path: from,
                            source: Source::Tree,
                            action: DeltaAction::Remove,
                        });
                    }
                    // Renamed into the wiki.
                    (false, true) => {
                        if !wiki_ignore.is_ignored(&to) {
                            out.push(PassDelta {
                                path: to,
                                source: Source::Tree,
                                action: DeltaAction::Add {
                                    oid: BlobOid(id.to_hex().to_string()),
                                    blob_bytes: None,
                                    stat_mtime_ns: None,
                                },
                            });
                        }
                    }
                    (false, false) => {}
                }
            }
        }
    }
    Ok(out)
}

/// Enumerate every markdown blob in the current HEAD tree as
/// `(repo-relative path, blob OID)`. Used by the bidirectional Tree
/// reconciliation when the wikiignore hash changed since the last refresh:
/// the incremental diff cannot observe a wikiignore-only commit, so the full
/// HEAD snapshot is needed to re-add files that became un-ignored without a
/// blob change. Returns an empty vector when HEAD is unborn.
pub fn head_tree_markdown_entries(repo: &gix::Repository) -> Result<Vec<(PathBuf, BlobOid)>> {
    let tree_id = match repo.head_tree_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    recurse_tree(repo, tree_id, &mut String::new(), &mut out)?;
    Ok(out)
}

fn recurse_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    prefix: &mut String,
    out: &mut Vec<(PathBuf, BlobOid)>,
) -> Result<()> {
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| anyhow::anyhow!("find tree {tree_id}: {e}"))?;
    for entry_ref in tree.iter() {
        let entry = entry_ref.map_err(|e| anyhow::anyhow!("decode tree entry: {e}"))?;
        let name = std::str::from_utf8(entry.filename())
            .map_err(|e| anyhow::anyhow!("tree entry name not utf-8: {e}"))?;
        let prev_len = prefix.len();
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(name);
        match entry.mode().kind() {
            gix::object::tree::EntryKind::Tree => {
                recurse_tree(repo, entry.object_id(), prefix, out)?;
            }
            gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable => {
                let path = PathBuf::from(&*prefix);
                if is_markdown(&path) {
                    out.push((path, BlobOid(entry.object_id().to_hex().to_string())));
                }
            }
            _ => {}
        }
        prefix.truncate(prev_len);
    }
    Ok(())
}

fn bstring_to_path(b: &gix::bstr::BString) -> PathBuf {
    PathBuf::from(b.to_string())
}

fn is_markdown(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}
