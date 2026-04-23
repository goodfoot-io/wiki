//! `git mesh` list / `git mesh <name>` show / `git mesh ls` — §10.4, §3.4.

use crate::cli::{parse_range_address, LsArgs, ShowArgs};
use crate::range::read_range;
use crate::{
    list_mesh_names, ls_all, ls_by_path, ls_by_path_range, mesh_commit_info, mesh_commit_info_at,
    mesh_log, read_mesh, read_mesh_at,
};
use anyhow::Result;

pub fn run_list(repo: &gix::Repository) -> Result<i32> {
    let names = list_mesh_names(repo)?;
    if names.is_empty() {
        println!("no meshes");
        return Ok(0);
    }
    for name in names {
        let m = read_mesh(repo, &name)?;
        let summary = m.message.lines().next().unwrap_or_default();
        println!("{name}\t{} ranges\t{summary}", m.ranges.len());
    }
    Ok(0)
}

pub fn run_show(repo: &gix::Repository, args: ShowArgs) -> Result<i32> {
    if args.log {
        let entries = mesh_log(repo, &args.name, args.limit)?;
        for info in entries {
            if args.oneline {
                println!("{} {}", short(&info.commit_oid), info.summary);
            } else {
                println!("commit {}", info.commit_oid);
                println!("Author: {} <{}>", info.author_name, info.author_email);
                println!("Date:   {}", info.author_date);
                println!();
                for line in info.message.trim_end_matches('\n').lines() {
                    println!("    {line}");
                }
                println!();
            }
        }
        return Ok(0);
    }

    let mesh = read_mesh_at(repo, &args.name, args.at.as_deref())?;
    let info = mesh_commit_info_at(repo, &args.name, args.at.as_deref())?;

    if args.oneline {
        for id in &mesh.ranges {
            let r = read_range(repo, id)?;
            let sha = if args.no_abbrev { r.anchor_sha.clone() } else { short(&r.anchor_sha).into() };
            println!("{sha}  {}#L{}-L{}", r.path, r.start, r.end);
        }
        return Ok(0);
    }

    println!("mesh {}", mesh.name);
    println!("commit {}", info.commit_oid);
    println!("Author: {} <{}>", info.author_name, info.author_email);
    println!("Date:   {}", info.author_date);
    println!();
    for line in mesh.message.trim_end_matches('\n').lines() {
        println!("    {line}");
    }
    println!();
    println!("Ranges ({}):", mesh.ranges.len());
    for id in &mesh.ranges {
        let r = read_range(repo, id)?;
        let sha = if args.no_abbrev { r.anchor_sha.clone() } else { short(&r.anchor_sha).into() };
        println!("    {sha}  {}#L{}-L{}", r.path, r.start, r.end);
    }

    // Consume unused field warning via bind.
    let _ = mesh_commit_info(repo, &args.name);
    let _ = args.format;
    Ok(0)
}

pub fn run_ls(repo: &gix::Repository, args: LsArgs) -> Result<i32> {
    let entries = match args.target {
        None => ls_all(repo)?,
        Some(t) => {
            if t.contains("#L") {
                let (path, s, e) = parse_range_address(&t)?;
                ls_by_path_range(repo, &path, s, e)?
            } else {
                ls_by_path(repo, &t)?
            }
        }
    };
    for e in entries {
        println!(
            "{}\t{}\t{}\t{}\t{}-{}",
            e.path, e.mesh_name, e.range_id, e.anchor_short, e.start, e.end
        );
    }
    Ok(0)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}
