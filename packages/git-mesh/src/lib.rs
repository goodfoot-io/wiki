pub mod types;

pub use types::*;
use anyhow::{anyhow, Result};
use chrono::Utc;
use gix::refs::transaction::PreviousValue;
use uuid::Uuid;

pub fn create_link(repo: &gix::Repository, input: CreateLinkInput) -> Result<(String, Link)> {
    let anchor_sha = repo
        .rev_parse_single(input.anchor_sha.as_deref().unwrap_or("HEAD"))?
        .detach();

    let [a, b] = input.sides;
    let side_a = build_side(repo, a, &anchor_sha)?;
    let side_b = build_side(repo, b, &anchor_sha)?;
    let mut pair = [side_a, side_b];
    pair.sort();

    let link = Link {
        anchor_sha: anchor_sha.to_string(),
        created_at: Utc::now().to_rfc3339(),
        sides: pair,
    };

    let id       = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let text     = serialize_link(&link);
    let blob_oid = repo.write_blob(text.as_bytes())?.detach();

    repo.reference(
        format!("refs/links/v1/{id}"),
        blob_oid,
        PreviousValue::MustNotExist,
        format!("create link {id}"),
    )?;
    Ok((id, link))
}

fn build_side(
    repo: &gix::Repository,
    spec: SideSpec,
    anchor_sha: &gix::ObjectId,
) -> Result<LinkSide> {
    let blob = resolve_blob(repo, anchor_sha, &spec.path, spec.start, spec.end)?;
    Ok(LinkSide {
        path: spec.path,
        start: spec.start,
        end: spec.end,
        blob: blob.to_string(),
        copy_detection: spec.copy_detection.unwrap_or(DEFAULT_COPY_DETECTION),
        ignore_whitespace: spec.ignore_whitespace.unwrap_or(DEFAULT_IGNORE_WHITESPACE),
    })
}

fn resolve_blob(
    repo: &gix::Repository,
    anchor_sha: &gix::ObjectId,
    path: &str,
    start: u32,
    end: u32,
) -> Result<gix::ObjectId> {
    let commit = repo.find_commit(*anchor_sha)?;
    let tree   = commit.tree()?;
    let entry  = tree
        .lookup_entry_by_path(path)?
        .ok_or_else(|| anyhow!("{path} not found at {anchor_sha}"))?;
    let blob   = repo.find_blob(entry.id())?;
    let mut lines = blob.data.iter().filter(|&&b| b == b'\n').count() as u32;
    if !blob.data.is_empty() && *blob.data.last().unwrap() != b'\n' {
        lines += 1;
    }
    if lines == 0 {
        lines = 1;
    }
    if start < 1 || end < start || end > lines {
        return Err(anyhow!("range {start}-{end} out of bounds for {path} ({lines} lines)"));
    }
    Ok(entry.id().detach())
}

pub fn commit_mesh(repo: &gix::Repository, input: CommitInput) -> Result<()> {
    let ref_name = format!("refs/meshes/v1/{}", input.name);
    let parent   = repo.try_find_reference(&ref_name)?
        .map(|mut r| r.peel_to_id()).transpose()?
        .map(|id| id.detach());

    if input.amend && (!input.adds.is_empty() || !input.removes.is_empty()) {
        return Err(anyhow!("--amend is incompatible with --link/--unlink"));
    }
    if parent.is_none() && input.adds.is_empty() {
        return Err(anyhow!("mesh `{}` does not exist; supply --link to create it",
                           input.name));
    }
    if !input.amend && input.adds.is_empty() && input.removes.is_empty() {
        return Err(anyhow!("nothing to commit"));
    }

    let mut links = match parent {
        Some(ref p) => read_mesh_links(repo, p)?,
        None        => Vec::new(),
    };

    for pair in &input.removes {
        let id = find_link_by_pair(repo, &links, pair)?;
        links.retain(|l| l != &id);
    }

    let anchor_sha = repo
        .rev_parse_single(input.anchor_sha.as_deref().unwrap_or("HEAD"))?
        .detach();
    for sides in input.adds {
        let (id, _) = create_link(repo, CreateLinkInput {
            sides,
            anchor_sha: Some(anchor_sha.to_string()),
            id: None,
        })?;
        ensure_pair_unique(repo, &links, &id)?;
        links.push(id);
    }

    write_mesh(
        repo,
        &input.name,
        &sort_and_dedupe(links),
        &normalize_message(&input.message),
        parent,
        input.amend,
    )
}

fn write_mesh(
    repo: &gix::Repository,
    name: &str,
    links: &[String],
    message: &str,
    expected_parent: Option<gix::ObjectId>,
    amend: bool,
) -> Result<()> {
    use gix::objs::{tree, Commit, Tree};
    let mut file = String::new();
    for id in links {
        file.push_str(id);
        file.push('\n');
    }
    let links_blob = repo.write_blob(file.as_bytes())?.detach();

    let tree_obj = Tree {
        entries: vec![tree::Entry {
            mode: tree::EntryKind::Blob.into(),
            filename: "links".into(),
            oid: links_blob,
        }],
    };
    let tree_id = repo.write_object(&tree_obj)?.detach();

    let new_parents: Vec<gix::ObjectId> = match (amend, expected_parent) {
        (true, Some(tip)) => repo.find_commit(tip)?.parent_ids()
            .map(|id| id.detach())
            .collect(),
        (true, None) => Vec::new(),
        (false, Some(tip)) => vec![tip],
        (false, None) => Vec::new(),
    };

    let author: gix::actor::Signature = repo.author().transpose()?.unwrap().into();
    let committer: gix::actor::Signature = repo.committer().transpose()?.unwrap().into();
    let commit = Commit {
        tree: tree_id,
        parents: new_parents.into(),
        author,
        committer,
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    let commit_id = repo.write_object(&commit)?.detach();

    let ref_name = format!("refs/meshes/v1/{name}");
    let previous = match expected_parent {
        Some(p) => PreviousValue::MustExistAndMatch(p.into()),
        None    => PreviousValue::MustNotExist,
    };
    repo.reference(ref_name, commit_id, previous, "mesh commit")?;
    Ok(())
}

fn canonical_pair(pair: &[RangeSpec; 2]) -> [RangeSpec; 2] {
    let mut p = pair.clone();
    p.sort();
    p
}

fn read_link(repo: &gix::Repository, id: &str) -> Result<Link> {
    let mut r = repo.find_reference(&format!("refs/links/v1/{id}"))?;
    let oid   = r.peel_to_id()?.detach();
    let blob  = repo.find_blob(oid)?;
    parse_link(std::str::from_utf8(&blob.data)?)
}

fn format_range(r: &RangeSpec) -> String {
    format!("{}#L{}-L{}", r.path, r.start, r.end)
}

fn find_link_by_pair(
    repo: &gix::Repository,
    link_ids: &[String],
    pair: &[RangeSpec; 2],
) -> Result<String> {
    let needle = canonical_pair(pair);
    for id in link_ids {
        let link = read_link(repo, id)?;
        let have = canonical_pair(&[
            RangeSpec { path: link.sides[0].path.clone(),
                        start: link.sides[0].start, end: link.sides[0].end },
            RangeSpec { path: link.sides[1].path.clone(),
                        start: link.sides[1].start, end: link.sides[1].end },
        ]);
        if have == needle {
            return Ok(id.clone());
        }
    }
    Err(anyhow!("no Link matching {}:{}",
                format_range(&pair[0]), format_range(&pair[1])))
}

fn ensure_pair_unique(
    repo: &gix::Repository,
    link_ids: &[String],
    new_id: &str,
) -> Result<()> {
    let new_link = read_link(repo, new_id)?;
    let needle = canonical_pair(&[
        RangeSpec { path: new_link.sides[0].path.clone(),
                    start: new_link.sides[0].start, end: new_link.sides[0].end },
        RangeSpec { path: new_link.sides[1].path.clone(),
                    start: new_link.sides[1].start, end: new_link.sides[1].end },
    ]);
    for id in link_ids {
        if id == new_id { continue; }
        let link = read_link(repo, id)?;
        let have = canonical_pair(&[
            RangeSpec { path: link.sides[0].path.clone(),
                        start: link.sides[0].start, end: link.sides[0].end },
            RangeSpec { path: link.sides[1].path.clone(),
                        start: link.sides[1].start, end: link.sides[1].end },
        ]);
        if have == needle {
            return Err(anyhow!("Mesh already contains a link for pair {}:{}",
                format_range(&needle[0]), format_range(&needle[1])));
        }
    }
    Ok(())
}

fn sort_and_dedupe(mut links: Vec<String>) -> Vec<String> {
    links.sort();
    links.dedup();
    links
}

fn normalize_message(msg: &str) -> String {
    msg.trim().to_string()
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

pub fn serialize_link(link: &Link) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "anchor {}", link.anchor_sha).unwrap();
    writeln!(out, "created {}", link.created_at).unwrap();
    for s in &link.sides {
        writeln!(out, "side {} {} {} {} {}\t{}",
                 s.start, s.end, s.blob,
                 copy_detection_str(s.copy_detection), s.ignore_whitespace,
                 s.path).unwrap();
    }
    out
}

fn copy_detection_str(cd: CopyDetection) -> &'static str {
    match cd {
        CopyDetection::Off => "off",
        CopyDetection::SameCommit => "same-commit",
        CopyDetection::AnyFileInCommit => "any-file-in-commit",
        CopyDetection::AnyFileInRepo => "any-file-in-repo",
    }
}

pub fn parse_link(text: &str) -> Result<Link> {
    let mut anchor_sha = String::new();
    let mut created_at = String::new();
    let mut sides = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("anchor ") {
            anchor_sha = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("created ") {
            created_at = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("side ") {
            let (fields, path) = rest.split_once('\t').ok_or_else(|| anyhow!("missing tab in side"))?;
            let parts: Vec<&str> = fields.split(' ').collect();
            if parts.len() != 5 {
                return Err(anyhow!("invalid side fields"));
            }
            sides.push(LinkSide {
                start: parts[0].parse()?,
                end: parts[1].parse()?,
                blob: parts[2].to_string(),
                copy_detection: parse_copy_detection(parts[3])?,
                ignore_whitespace: parts[4].parse()?,
                path: path.to_string(),
            });
        }
    }
    if sides.len() != 2 {
        return Err(anyhow!("link must have exactly 2 sides"));
    }
    let mut sides_arr = [sides[0].clone(), sides[1].clone()];
    sides_arr.sort();
    Ok(Link {
        anchor_sha,
        created_at,
        sides: sides_arr,
    })
}

fn parse_copy_detection(s: &str) -> Result<CopyDetection> {
    match s {
        "off" => Ok(CopyDetection::Off),
        "same-commit" => Ok(CopyDetection::SameCommit),
        "any-file-in-commit" => Ok(CopyDetection::AnyFileInCommit),
        "any-file-in-repo" => Ok(CopyDetection::AnyFileInRepo),
        _ => Err(anyhow!("invalid copy detection {}", s)),
    }
}

pub fn read_mesh_links(repo: &gix::Repository, commit_id: &gix::ObjectId) -> Result<Vec<String>> {
    let commit = repo.find_commit(*commit_id)?;
    let tree   = commit.tree()?;
    let entry  = tree
        .lookup_entry_by_path("links")?
        .ok_or_else(|| anyhow!("mesh commit {} has no `links` file", commit_id))?;
    let blob   = repo.find_blob(entry.id())?;
    let text   = std::str::from_utf8(&blob.data)?;
    Ok(text.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect())
}
