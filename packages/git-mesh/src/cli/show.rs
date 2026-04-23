use crate::cli::{
    abbreviate_oid, format_link_pair, index_links_by_pair, maybe_abbreviate,
    print_indented_message, stored_links_sorted, PrintOptions,
};
use anyhow::{Result, anyhow};
use git_mesh::{
    list_mesh_names, mesh_commit_info_at, mesh_log, read_mesh_at, show_mesh, MeshCommitInfo,
    MeshStored,
};
use std::collections::BTreeSet;

pub(crate) fn print_mesh_list(repo: &gix::Repository) -> Result<()> {
    let names = list_mesh_names(repo)?;
    if names.is_empty() {
        println!("no meshes");
        return Ok(());
    }

    for name in names {
        let mesh = show_mesh(repo, &name)?;
        let summary = mesh.message.lines().next().unwrap_or_default();
        println!("{name}\t{} links\t{summary}", mesh.links.len());
    }

    Ok(())
}

pub(crate) fn print_mesh(mesh: &MeshStored, info: &MeshCommitInfo, options: PrintOptions) {
    if options.oneline {
        for link in stored_links_sorted(&mesh.links) {
            println!(
                "{}  {}",
                maybe_abbreviate(&link.anchor_sha, options.no_abbrev),
                format_link_pair(link)
            );
        }
        return;
    }

    println!("mesh {}", mesh.name);
    println!("commit {}", info.commit_oid);
    println!("Author: {} <{}>", info.author_name, info.author_email);
    println!("Date:   {}", info.author_date);
    println!();
    print_indented_message(&mesh.message);
    println!();
    println!("Links ({}):", mesh.links.len());
    for link in stored_links_sorted(&mesh.links) {
        println!(
            "    {}  {}",
            maybe_abbreviate(&link.anchor_sha, options.no_abbrev),
            format_link_pair(link)
        );
    }
}

pub(crate) fn print_mesh_format(
    mesh: &MeshStored,
    info: &MeshCommitInfo,
    format: &str,
    options: PrintOptions,
) -> Result<()> {
    // §10.2: `--format=<fmt>` is a git-log-style format string.
    //
    // Supported tokens (all match `git log --format` semantics except `%M`/`%L`/`%l`,
    // which are reserved for mesh-specific data in tokens that git leaves undefined
    // at the top level):
    //   %H   full commit sha of the mesh tip
    //   %h   abbreviated commit sha (honors --no-abbrev)
    //   %s   subject (first line of the mesh message)
    //   %B   raw body (full commit message, subject + body)
    //   %an  author name
    //   %ae  author email
    //   %ad  author date
    //   %n   newline
    //   %%   literal percent
    //   %M   mesh name          (git-specific: `%m` is left/right mark — reserved)
    //   %L   links block        (git-specific: undefined at top level in git-log)
    //   %l   link count         (git-specific: undefined at top level in git-log)
    //
    // Any other `%X` is rejected so format strings stay explicit.
    let mut output = String::new();
    let mut chars = format.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }

        match chars.next() {
            Some('%') => output.push('%'),
            Some('n') => output.push('\n'),
            Some('M') => output.push_str(&mesh.name),
            Some('H') => output.push_str(&info.commit_oid),
            Some('h') => output.push_str(maybe_abbreviate(&info.commit_oid, options.no_abbrev)),
            Some('s') => output.push_str(&info.summary),
            Some('B') => output.push_str(&mesh.message),
            Some('a') => match chars.next() {
                Some('n') => output.push_str(&info.author_name),
                Some('e') => output.push_str(&info.author_email),
                Some('d') => output.push_str(&info.author_date),
                Some(other) => return Err(anyhow!("unsupported format token `%a{other}`")),
                None => return Err(anyhow!("dangling format token `%a`")),
            },
            Some('L') => output.push_str(&format_links_block(mesh, options.no_abbrev)),
            Some('l') => output.push_str(&mesh.links.len().to_string()),
            Some(other) => return Err(anyhow!("unsupported format token `%{other}`")),
            None => return Err(anyhow!("dangling trailing `%` in format string")),
        }
    }

    print!("{output}");
    if !output.ends_with('\n') {
        println!();
    }
    Ok(())
}

pub(crate) fn print_mesh_log(
    repo: &gix::Repository,
    name: &str,
    oneline: bool,
    limit: Option<usize>,
) -> Result<()> {
    // §6.4 / §10.2: `git mesh <name> --log` mirrors `git log refs/meshes/v1/<name>`.
    // Default rendering shows header + full commit message (subject + body) indented
    // four spaces, matching `git log`'s default. `--oneline` collapses to one line.
    let entries = mesh_log(repo, name, limit)?;
    for info in entries {
        if oneline {
            println!("{} {}", abbreviate_oid(&info.commit_oid), info.summary);
        } else {
            println!("commit {}", info.commit_oid);
            println!("Author: {} <{}>", info.author_name, info.author_email);
            println!("Date:   {}", info.author_date);
            println!();
            print_indented_message(info.message.trim_end_matches('\n'));
            println!();
        }
    }
    Ok(())
}

pub(crate) fn print_mesh_diff(
    repo: &gix::Repository,
    name: &str,
    revision_range: &str,
) -> Result<()> {
    let (left_rev, right_rev) = revision_range
        .split_once("..")
        .ok_or_else(|| anyhow!("invalid diff range `{revision_range}`; expected <rev>..<rev>"))?;
    anyhow::ensure!(
        !left_rev.is_empty() && !right_rev.is_empty(),
        "invalid diff range `{revision_range}`; expected <rev>..<rev>"
    );

    let left = read_mesh_at(repo, name, Some(left_rev))?;
    let right = read_mesh_at(repo, name, Some(right_rev))?;
    let left_info = mesh_commit_info_at(repo, name, Some(left_rev))?;
    let right_info = mesh_commit_info_at(repo, name, Some(right_rev))?;

    let left_links = index_links_by_pair(&left.links);
    let right_links = index_links_by_pair(&right.links);
    let all_pairs: BTreeSet<_> = left_links
        .keys()
        .cloned()
        .chain(right_links.keys().cloned())
        .collect();

    println!("mesh {}", name);
    println!("diff {}..{}", left_info.commit_oid, right_info.commit_oid);
    println!();

    for pair in all_pairs {
        match (left_links.get(&pair), right_links.get(&pair)) {
            (None, Some(link)) => {
                println!("+ {} @ {}", pair, abbreviate_oid(&link.anchor_sha));
            }
            (Some(link), None) => {
                println!("- {} @ {}", pair, abbreviate_oid(&link.anchor_sha));
            }
            (Some(left_link), Some(right_link))
                if left_link.anchor_sha != right_link.anchor_sha =>
            {
                println!(
                    "~ {} @ {} -> {}",
                    pair,
                    abbreviate_oid(&left_link.anchor_sha),
                    abbreviate_oid(&right_link.anchor_sha)
                );
            }
            _ => {}
        }
    }

    Ok(())
}

pub(crate) fn format_links_block(mesh: &MeshStored, no_abbrev: bool) -> String {
    stored_links_sorted(&mesh.links)
        .into_iter()
        .map(|link| {
            format!(
                "{}  {}",
                maybe_abbreviate(&link.anchor_sha, no_abbrev),
                format_link_pair(link)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

