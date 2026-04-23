use anyhow::Result;
use git_mesh_legacy::{remove_mesh, rename_mesh, restore_mesh, validate_mesh_name};

pub(crate) fn run_rm(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let name = sub_matches.get_one::<String>("name").unwrap();
    remove_mesh(repo, name)?;
    println!("deleted refs/meshes/v1/{name}");
    Ok(())
}

pub(crate) fn run_mv(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let old_name = sub_matches.get_one::<String>("old").unwrap();
    let new_name = sub_matches.get_one::<String>("new").unwrap();
    validate_mesh_name(new_name)?;
    rename_mesh(repo, old_name, new_name, sub_matches.get_flag("keep"))?;
    if sub_matches.get_flag("keep") {
        println!("copied refs/meshes/v1/{old_name} to refs/meshes/v1/{new_name}");
    } else {
        println!("renamed refs/meshes/v1/{old_name} to refs/meshes/v1/{new_name}");
    }
    Ok(())
}

pub(crate) fn run_restore(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let name = sub_matches.get_one::<String>("name").unwrap();
    let commit_ish = sub_matches.get_one::<String>("commit-ish").unwrap();
    restore_mesh(repo, name, commit_ish)?;
    println!("restored refs/meshes/v1/{name} from {commit_ish}");
    Ok(())
}
