use anyhow::Result;
use git_mesh::{fetch_mesh_refs, list_mesh_names, push_mesh_refs, read_link, read_mesh_at};

pub(crate) fn run_fetch(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let remote = fetch_mesh_refs(
        repo,
        sub_matches.get_one::<String>("remote").map(String::as_str),
    )?;
    println!("fetched mesh refs from {remote}");
    Ok(())
}

pub(crate) fn run_push(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let remote = push_mesh_refs(
        repo,
        sub_matches.get_one::<String>("remote").map(String::as_str),
    )?;
    println!("pushed mesh refs to {remote}");
    Ok(())
}

pub(crate) fn run_doctor(repo: &gix::Repository) -> Result<i32> {
    let issues = run_doctor_collect(repo);
    if issues.is_empty() {
        println!("mesh doctor: ok");
        Ok(0)
    } else {
        println!("mesh doctor: found {} issue(s)", issues.len());
        for issue in issues {
            println!("{issue}");
        }
        Ok(1)
    }
}

fn run_doctor_collect(repo: &gix::Repository) -> Vec<String> {
    let mut issues = Vec::new();
    let Ok(names) = list_mesh_names(repo) else {
        issues.push("failed to list mesh refs".to_string());
        return issues;
    };

    for name in names {
        match read_mesh_at(repo, &name, None) {
            Ok(mesh) => {
                for link in mesh.links {
                    if let Err(error) = read_link(repo, &link.id) {
                        issues.push(format!(
                            "mesh `{name}` link `{}` is unreadable: {error:#}",
                            link.id
                        ));
                    }
                }
            }
            Err(error) => issues.push(format!("mesh `{name}` is unreadable: {error:#}")),
        }
    }

    issues
}
