//! Pass 1 — diff `HEAD^{tree}` against the previously-recorded tree OID
//! (or the empty tree for first-run / unborn HEAD), filtered to `.md`
//! paths.

use std::path::PathBuf;

use anyhow::Result;

use crate::index::{BlobOid, Source};

use super::{DeltaAction, PassDelta};

/// SHA-1 of the empty tree, used as the prior tree when `last_head_tree_oid`
/// is `None`.
const EMPTY_TREE_OID: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub fn pass_tree(
    repo: &gix::Repository,
    last_head_tree_oid: Option<gix::ObjectId>,
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
                out.push(PassDelta {
                    path,
                    source: Source::Tree,
                    action: DeltaAction::Add {
                        oid: BlobOid(id.to_hex().to_string()),
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
                out.push(PassDelta {
                    path,
                    source: Source::Tree,
                    action: DeltaAction::Add {
                        oid: BlobOid(id.to_hex().to_string()),
                    },
                });
            }
            gix::object::tree::diff::ChangeDetached::Rewrite {
                source_location,
                location,
                id,
                ..
            } => {
                let from = bstring_to_path(&source_location);
                let to = bstring_to_path(&location);
                if !is_markdown(&to) && !is_markdown(&from) {
                    continue;
                }
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
    }
    Ok(out)
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
